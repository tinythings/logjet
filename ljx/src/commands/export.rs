//! Top-level file export commands for `ljx`.
//!
//! Ticket 101 adds a CLI path for exporting one `.logjet` input into another
//! format without going through the interactive TUI.

use std::io::Write;
use std::path::Path;

use chrono::{TimeZone, Utc};
use logjet::{LogjetReader, OwnedRecord, RecordType};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::AnyValue;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use prost::Message;
use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::error::{Error, Result};
use crate::exporter::ExporterRegistry;
use crate::input::{InputHandle, open_output_with_policy};

/// Run the top-level export flow for one input file.
pub fn run(format: &str, input: &Path, output: &Path, force: bool) -> Result<()> {
    match format {
        "ndjson" => run_ndjson(input, output, force),
        other => {
            let registry = ExporterRegistry::discover();
            let Some(plugin) = registry.plugin(other) else {
                return Err(registry.unknown_format_error(other));
            };
            plugin.export(input, output, force, &[])
        }
    }
}

fn run_ndjson(input: &Path, output: &Path, force: bool) -> Result<()> {
    let input = InputHandle::open(input)?;
    let mut reader = LogjetReader::new(input.into_buf_reader());
    let mut output = open_output_with_policy(output, force)?;

    while let Some(record) = reader.next_record()? {
        for row in export_ndjson_objects(&record) {
            serde_json::to_writer(&mut output, &row).map_err(|err| Error::Usage(format!("failed to serialise NDJSON row: {err}")))?;
            output.write_all(b"\n")?;
        }
    }

    output.flush()?;
    Ok(())
}

fn export_ndjson_objects(record: &OwnedRecord) -> Vec<JsonValue> {
    if record.record_type != RecordType::Logs {
        let mut obj = JsonMap::new();
        obj.insert("record_type".to_string(), JsonValue::String(record_kind_label(record.record_type).to_string()));
        obj.insert("timestamp".to_string(), JsonValue::String(format_timestamp(record.ts_unix_ns)));
        obj.insert("payload".to_string(), JsonValue::String(String::from_utf8_lossy(&record.payload).to_string()));
        return vec![JsonValue::Object(obj)];
    }

    let Ok(batch) = ExportLogsServiceRequest::decode(record.payload.as_slice()) else {
        let mut obj = JsonMap::new();
        obj.insert("timestamp".to_string(), JsonValue::String(format_timestamp(record.ts_unix_ns)));
        obj.insert("payload".to_string(), JsonValue::String(String::from_utf8_lossy(&record.payload).to_string()));
        return vec![JsonValue::Object(obj)];
    };

    let mut out = Vec::new();
    for resource_logs in &batch.resource_logs {
        let resource_attrs = resource_logs.resource.as_ref().map(|r| &r.attributes).map(Vec::as_slice).unwrap_or(&[]);
        for scope_logs in &resource_logs.scope_logs {
            let scope_attrs = scope_logs.scope.as_ref().map(|s| s.attributes.as_slice()).unwrap_or(&[]);
            for log_record in &scope_logs.log_records {
                let mut obj = JsonMap::new();
                obj.insert(
                    "body".to_string(),
                    JsonValue::String(log_record.body.as_ref().map(|v| format_any_value(Some(v))).filter(|s| !s.is_empty()).unwrap_or_default()),
                );
                obj.insert("timestamp".to_string(), JsonValue::String(format_timestamp(log_record.time_unix_nano.max(record.ts_unix_ns))));
                if !log_record.event_name.is_empty() {
                    obj.insert("event_name".to_string(), JsonValue::String(log_record.event_name.clone()));
                }
                flatten_otlp_attrs_into_json(&mut obj, resource_attrs);
                flatten_otlp_attrs_into_json(&mut obj, scope_attrs);
                flatten_otlp_attrs_into_json(&mut obj, &log_record.attributes);
                out.push(JsonValue::Object(obj));
            }
        }
    }
    out
}

fn flatten_otlp_attrs_into_json(target: &mut JsonMap<String, JsonValue>, attrs: &[opentelemetry_proto::tonic::common::v1::KeyValue]) {
    for attr in attrs {
        let key = attr.key.replace('.', "_");
        if target.contains_key(&key) {
            continue;
        }
        let Some(value) = attr.value.as_ref() else {
            continue;
        };
        if let Some(json) = any_value_to_json(value) {
            target.insert(key, json);
        }
    }
}

fn any_value_to_json(value: &AnyValue) -> Option<JsonValue> {
    match &value.value {
        Some(Value::StringValue(text)) => Some(JsonValue::String(text.clone())),
        Some(Value::BoolValue(flag)) => Some(JsonValue::Bool(*flag)),
        Some(Value::IntValue(number)) => Some(JsonValue::Number((*number).into())),
        Some(Value::DoubleValue(number)) => serde_json::Number::from_f64(*number).map(JsonValue::Number),
        Some(Value::BytesValue(bytes)) => Some(JsonValue::String(format!("<{} bytes>", bytes.len()))),
        Some(Value::ArrayValue(array)) => Some(JsonValue::Array(array.values.iter().filter_map(any_value_to_json).collect())),
        Some(Value::KvlistValue(map)) => Some(JsonValue::Object(
            map.values
                .iter()
                .filter_map(|item| item.value.as_ref().and_then(any_value_to_json).map(|inner| (item.key.clone().replace('.', "_"), inner)))
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
