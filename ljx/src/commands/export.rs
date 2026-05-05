//! Top-level file export commands for `ljx`.
//!
//! Ticket 101 adds a CLI path for exporting one `.logjet` input into another
//! format without going through the interactive TUI.

use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{TimeZone, Utc};
use logjet::{LogjetReader, OwnedRecord, RecordType};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::AnyValue;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::logs::v1::LogRecord;
use prost::Message;
use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::dataset::Dataset;
use crate::error::{Error, Result};
use crate::exporter::ExporterRegistry;
use crate::input::{InputHandle, open_output_with_policy};
use crate::predicate::RecordPredicate;

/// Run the top-level export flow for one input file.
pub fn run(format: &str, input: &Path, output: &Path, force: bool, fields: &[String]) -> Result<()> {
    match format {
        "ndjson" => run_ndjson(input, output, force, fields),
        other => {
            if !fields.is_empty() {
                return Err(Error::Usage(format!("--fields is only supported with ndjson output, not {other}")));
            }
            let registry = ExporterRegistry::discover();
            let Some(plugin) = registry.plugin(other) else {
                return Err(registry.unknown_format_error(other));
            };
            plugin.export(input, output, force, &[])
        }
    }
}

pub fn run_query_ndjson_multi(inputs: &[PathBuf], predicate: &RecordPredicate, fields: &[String], preview_bytes: Option<usize>) -> Result<()> {
    let mut output = open_output_with_policy(Path::new("-"), true)?;
    run_query_ndjson_multi_with_writer(inputs, predicate, fields, preview_bytes, &mut output)?;
    output.flush()?;
    Ok(())
}

pub(crate) fn run_query_ndjson_multi_with_writer(
    inputs: &[PathBuf], predicate: &RecordPredicate, fields: &[String], preview_bytes: Option<usize>, output: &mut impl Write,
) -> Result<()> {
    let dataset = Dataset::from_inputs(inputs)?;

    for entry in dataset.entries() {
        let input = InputHandle::open(entry.path.as_path())?;
        let mut reader = LogjetReader::new(input.into_buf_reader());
        while let Some(record) = reader.next_record()? {
            if !predicate.matches(&record) {
                continue;
            }
            for row in export_ndjson_objects_with_preview(&record, fields, preview_bytes) {
                serde_json::to_writer(&mut *output, &row).map_err(|err| Error::Usage(format!("failed to serialise NDJSON row: {err}")))?;
                output.write_all(b"\n")?;
            }
        }
    }

    Ok(())
}

fn run_ndjson(input: &Path, output: &Path, force: bool, fields: &[String]) -> Result<()> {
    let input = InputHandle::open(input)?;
    let mut reader = LogjetReader::new(input.into_buf_reader());
    let mut output = open_output_with_policy(output, force)?;

    while let Some(record) = reader.next_record()? {
        for row in export_ndjson_objects(&record, fields) {
            serde_json::to_writer(&mut output, &row).map_err(|err| Error::Usage(format!("failed to serialise NDJSON row: {err}")))?;
            output.write_all(b"\n")?;
        }
    }

    output.flush()?;
    Ok(())
}

pub(crate) fn export_ndjson_objects(record: &OwnedRecord, fields: &[String]) -> Vec<JsonValue> {
    export_ndjson_objects_with_preview(record, fields, None)
}

pub(crate) fn export_ndjson_objects_with_preview(record: &OwnedRecord, fields: &[String], preview_bytes: Option<usize>) -> Vec<JsonValue> {
    if record.record_type != RecordType::Logs {
        let mut obj = JsonMap::new();
        obj.insert("record_type".to_string(), JsonValue::String(record_kind_label(record.record_type).to_string()));
        obj.insert("timestamp".to_string(), JsonValue::String(format_timestamp(record.ts_unix_ns)));
        obj.insert("payload".to_string(), JsonValue::String(truncate_preview(&String::from_utf8_lossy(&record.payload), preview_bytes)));
        return vec![select_json_fields(obj, fields)];
    }

    let Ok(batch) = ExportLogsServiceRequest::decode(record.payload.as_slice()) else {
        let mut obj = JsonMap::new();
        obj.insert("timestamp".to_string(), JsonValue::String(format_timestamp(record.ts_unix_ns)));
        obj.insert("payload".to_string(), JsonValue::String(truncate_preview(&String::from_utf8_lossy(&record.payload), preview_bytes)));
        return vec![select_json_fields(obj, fields)];
    };

    let mut out = Vec::new();
    for resource_logs in &batch.resource_logs {
        let resource_attrs = resource_logs.resource.as_ref().map(|r| &r.attributes).map(Vec::as_slice).unwrap_or(&[]);
        for scope_logs in &resource_logs.scope_logs {
            let scope_attrs = scope_logs.scope.as_ref().map(|s| s.attributes.as_slice()).unwrap_or(&[]);
            for log_record in &scope_logs.log_records {
                let mut obj = JsonMap::new();
                if let Some(scope) = &scope_logs.scope {
                    if !scope.name.is_empty() {
                        obj.insert("scope_name".to_string(), JsonValue::String(scope.name.clone()));
                    }
                    if !scope.version.is_empty() {
                        obj.insert("scope_version".to_string(), JsonValue::String(scope.version.clone()));
                    }
                }
                insert_otlp_log_fields_with_preview(&mut obj, log_record, record.ts_unix_ns, preview_bytes);
                flatten_otlp_attrs_into_json(&mut obj, resource_attrs, preview_bytes);
                flatten_otlp_attrs_into_json(&mut obj, scope_attrs, preview_bytes);
                flatten_otlp_attrs_into_json(&mut obj, &log_record.attributes, preview_bytes);
                out.push(select_json_fields(obj, fields));
            }
        }
    }
    out
}

fn insert_otlp_log_fields_with_preview(
    target: &mut JsonMap<String, JsonValue>, log_record: &LogRecord, fallback_ts_unix_ns: u64, preview_bytes: Option<usize>,
) {
    target.insert(
        "body".to_string(),
        JsonValue::String(truncate_preview(&log_record.body.as_ref().map(|v| format_any_value(Some(v))).filter(|s| !s.is_empty()).unwrap_or_default(), preview_bytes)),
    );
    target.insert("timestamp".to_string(), JsonValue::String(format_timestamp(log_record.time_unix_nano.max(fallback_ts_unix_ns))));
    if log_record.observed_time_unix_nano > 0 {
        target.insert("observed_timestamp".to_string(), JsonValue::String(format_timestamp(log_record.observed_time_unix_nano)));
    }
    if log_record.severity_number != 0 {
        target.insert("severity_number".to_string(), JsonValue::Number(log_record.severity_number.into()));
    }
    if !log_record.severity_text.is_empty() {
        target.insert("severity_text".to_string(), JsonValue::String(log_record.severity_text.clone()));
    }
    if log_record.flags != 0 {
        target.insert("flags".to_string(), JsonValue::Number(log_record.flags.into()));
    }
    if !log_record.event_name.is_empty() {
        target.insert("event_name".to_string(), JsonValue::String(log_record.event_name.clone()));
    }
    if !log_record.trace_id.is_empty() {
        target.insert("trace_id".to_string(), JsonValue::String(hex_encode(&log_record.trace_id)));
    }
    if !log_record.span_id.is_empty() {
        target.insert("span_id".to_string(), JsonValue::String(hex_encode(&log_record.span_id)));
    }
}

fn truncate_preview(value: &str, preview_bytes: Option<usize>) -> String {
    let Some(limit) = preview_bytes else {
        return value.to_string();
    };
    if limit == 0 {
        return String::new();
    }
    let mut out = String::new();
    for ch in value.chars() {
        let next_len = out.len() + ch.len_utf8();
        if next_len > limit {
            break;
        }
        out.push(ch);
    }
    if value.len() > out.len() {
        out.push_str("...");
    }
    out
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn select_json_fields(mut obj: JsonMap<String, JsonValue>, fields: &[String]) -> JsonValue {
    if fields.is_empty() {
        return JsonValue::Object(obj);
    }

    let mut selected = JsonMap::new();
    for field in fields {
        if let Some(value) = obj.remove(field) {
            selected.insert(field.clone(), value);
        }
    }
    JsonValue::Object(selected)
}

fn flatten_otlp_attrs_into_json(
    target: &mut JsonMap<String, JsonValue>, attrs: &[opentelemetry_proto::tonic::common::v1::KeyValue], preview_bytes: Option<usize>,
) {
    for attr in attrs {
        let key = attr.key.replace('.', "_");
        if target.contains_key(&key) {
            continue;
        }
        let Some(value) = attr.value.as_ref() else {
            continue;
        };
        if let Some(json) = any_value_to_json(value, preview_bytes) {
            target.insert(key, json);
        }
    }
}

fn any_value_to_json(value: &AnyValue, preview_bytes: Option<usize>) -> Option<JsonValue> {
    match &value.value {
        Some(Value::StringValue(text)) => Some(JsonValue::String(truncate_preview(text, preview_bytes))),
        Some(Value::BoolValue(flag)) => Some(JsonValue::Bool(*flag)),
        Some(Value::IntValue(number)) => Some(JsonValue::Number((*number).into())),
        Some(Value::DoubleValue(number)) => serde_json::Number::from_f64(*number).map(JsonValue::Number),
        Some(Value::BytesValue(bytes)) => Some(JsonValue::String(truncate_preview(&format!("<{} bytes>", bytes.len()), preview_bytes))),
        Some(Value::ArrayValue(array)) => Some(JsonValue::Array(array.values.iter().filter_map(|value| any_value_to_json(value, preview_bytes)).collect())),
        Some(Value::KvlistValue(map)) => Some(JsonValue::Object(
            map.values
                .iter()
                .filter_map(|item| {
                    item.value
                        .as_ref()
                        .and_then(|inner| any_value_to_json(inner, preview_bytes))
                        .map(|inner| (item.key.clone().replace('.', "_"), inner))
                })
                .collect(),
        )),
        None => None,
    }
}

fn format_any_value(value: Option<&AnyValue>) -> String {
    let Some(value) = value else {
        return "null".to_string();
    };
    match &value.value {
        Some(Value::StringValue(text)) => text.clone(),
        Some(Value::BoolValue(flag)) => flag.to_string(),
        Some(Value::IntValue(number)) => number.to_string(),
        Some(Value::DoubleValue(number)) => number.to_string(),
        Some(Value::BytesValue(bytes)) => format!("<{} bytes>", bytes.len()),
        Some(Value::ArrayValue(array)) => format!("<array:{}>", array.values.len()),
        Some(Value::KvlistValue(map)) => format!("<map:{}>", map.values.len()),
        None => "null".to_string(),
    }
}

fn record_kind_label(record_type: RecordType) -> &'static str {
    match record_type {
        RecordType::Logs => "logs",
        RecordType::Metrics => "metrics",
        RecordType::Traces => "traces",
    }
}

fn format_timestamp(ts_unix_ns: u64) -> String {
    let secs = (ts_unix_ns / 1_000_000_000) as i64;
    let nanos = (ts_unix_ns % 1_000_000_000) as u32;
    match Utc.timestamp_opt(secs, nanos).single() {
        Some(ts) => ts.format("%Y-%m-%d %H:%M:%S.%f UTC").to_string(),
        None => ts_unix_ns.to_string(),
    }
}

#[cfg(test)]
#[path = "../../tests/unit/commands/top_level_query_ut.rs"]
mod top_level_query_ut;

