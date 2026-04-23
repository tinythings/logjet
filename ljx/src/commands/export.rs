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
use opentelemetry_proto::tonic::logs::v1::LogRecord;
use prost::Message;
use serde_json::{Map as JsonMap, Value as JsonValue};

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

pub fn run_query_ndjson(input: &Path, predicate: &RecordPredicate, fields: &[String]) -> Result<()> {
    let input = InputHandle::open(input)?;
    let mut reader = LogjetReader::new(input.into_buf_reader());
    let mut output = open_output_with_policy(Path::new("-"), true)?;

    while let Some(record) = reader.next_record()? {
        if !predicate.matches(&record) {
            continue;
        }
        for row in export_ndjson_objects(&record, fields) {
            serde_json::to_writer(&mut output, &row).map_err(|err| Error::Usage(format!("failed to serialise NDJSON row: {err}")))?;
            output.write_all(b"\n")?;
        }
    }

    output.flush()?;
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
    if record.record_type != RecordType::Logs {
        let mut obj = JsonMap::new();
        obj.insert("record_type".to_string(), JsonValue::String(record_kind_label(record.record_type).to_string()));
        obj.insert("timestamp".to_string(), JsonValue::String(format_timestamp(record.ts_unix_ns)));
        obj.insert("payload".to_string(), JsonValue::String(String::from_utf8_lossy(&record.payload).to_string()));
        return vec![select_json_fields(obj, fields)];
    }

    let Ok(batch) = ExportLogsServiceRequest::decode(record.payload.as_slice()) else {
        let mut obj = JsonMap::new();
        obj.insert("timestamp".to_string(), JsonValue::String(format_timestamp(record.ts_unix_ns)));
        obj.insert("payload".to_string(), JsonValue::String(String::from_utf8_lossy(&record.payload).to_string()));
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
                insert_otlp_log_fields(&mut obj, log_record, record.ts_unix_ns);
                flatten_otlp_attrs_into_json(&mut obj, resource_attrs);
                flatten_otlp_attrs_into_json(&mut obj, scope_attrs);
                flatten_otlp_attrs_into_json(&mut obj, &log_record.attributes);
                out.push(select_json_fields(obj, fields));
            }
        }
    }
    out
}

fn insert_otlp_log_fields(target: &mut JsonMap<String, JsonValue>, log_record: &LogRecord, fallback_ts_unix_ns: u64) {
    target.insert(
        "body".to_string(),
        JsonValue::String(log_record.body.as_ref().map(|v| format_any_value(Some(v))).filter(|s| !s.is_empty()).unwrap_or_default()),
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

#[cfg(test)]
mod export_utst {
    use super::{export_ndjson_objects, select_json_fields};
    use logjet::{OwnedRecord, RecordType};
    use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
    use opentelemetry_proto::tonic::common::v1::any_value::Value;
    use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope, KeyValue};
    use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
    use opentelemetry_proto::tonic::resource::v1::Resource;
    use prost::Message;
    use serde_json::{Map as JsonMap, Value as JsonValue};

    #[test]
    fn select_json_fields_keeps_all_keys_when_unset() {
        let mut obj = JsonMap::new();
        obj.insert("body".to_string(), JsonValue::String("hello".to_string()));
        obj.insert("timestamp".to_string(), JsonValue::String("now".to_string()));

        let JsonValue::Object(selected) = select_json_fields(obj, &[]) else { panic!("expected object") };
        assert_eq!(selected.len(), 2);
        assert_eq!(selected.get("body"), Some(&JsonValue::String("hello".to_string())));
        assert_eq!(selected.get("timestamp"), Some(&JsonValue::String("now".to_string())));
    }

    #[test]
    fn select_json_fields_filters_to_requested_subset() {
        let mut obj = JsonMap::new();
        obj.insert("body".to_string(), JsonValue::String("hello".to_string()));
        obj.insert("timestamp".to_string(), JsonValue::String("now".to_string()));
        obj.insert("service_name".to_string(), JsonValue::String("svc".to_string()));

        let fields = vec!["service_name".to_string(), "body".to_string()];
        let JsonValue::Object(selected) = select_json_fields(obj, &fields) else { panic!("expected object") };
        assert_eq!(selected.len(), 2);
        assert_eq!(selected.get("service_name"), Some(&JsonValue::String("svc".to_string())));
        assert_eq!(selected.get("body"), Some(&JsonValue::String("hello".to_string())));
    }

    #[test]
    fn export_ndjson_objects_includes_core_otlp_log_fields() {
        let batch = ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                resource: Some(Resource {
                    attributes: vec![KeyValue {
                        key: "service.name".to_string(),
                        value: Some(AnyValue { value: Some(Value::StringValue("demo-svc".to_string())) }),
                    }],
                    dropped_attributes_count: 0,
                    entity_refs: Vec::new(),
                }),
                scope_logs: vec![ScopeLogs {
                    scope: Some(InstrumentationScope {
                        name: "demo.scope".to_string(),
                        version: "1.2.3".to_string(),
                        attributes: Vec::new(),
                        dropped_attributes_count: 0,
                    }),
                    log_records: vec![LogRecord {
                        time_unix_nano: 1_700_000_000_000_000_000,
                        observed_time_unix_nano: 1_700_000_000_000_000_123,
                        severity_number: 9,
                        severity_text: "INFO".to_string(),
                        body: Some(AnyValue { value: Some(Value::StringValue("hello".to_string())) }),
                        attributes: Vec::new(),
                        dropped_attributes_count: 0,
                        flags: 3,
                        trace_id: vec![0xaa, 0xbb, 0xcc, 0xdd],
                        span_id: vec![0x11, 0x22],
                        event_name: "demo.event".to_string(),
                    }],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        };
        let record = OwnedRecord { record_type: RecordType::Logs, seq: 1, ts_unix_ns: 1_700_000_000_000_000_000, payload: batch.encode_to_vec() };

        let docs = export_ndjson_objects(&record, &[]);
        let Some(obj) = docs.first().and_then(JsonValue::as_object) else { panic!("expected object") };
        assert_eq!(obj.get("body"), Some(&JsonValue::String("hello".to_string())));
        assert_eq!(obj.get("severity_text"), Some(&JsonValue::String("INFO".to_string())));
        assert_eq!(obj.get("severity_number"), Some(&JsonValue::Number(9.into())));
        assert_eq!(obj.get("event_name"), Some(&JsonValue::String("demo.event".to_string())));
        assert_eq!(obj.get("scope_name"), Some(&JsonValue::String("demo.scope".to_string())));
        assert_eq!(obj.get("scope_version"), Some(&JsonValue::String("1.2.3".to_string())));
        assert_eq!(obj.get("service_name"), Some(&JsonValue::String("demo-svc".to_string())));
        assert_eq!(obj.get("trace_id"), Some(&JsonValue::String("aabbccdd".to_string())));
        assert_eq!(obj.get("span_id"), Some(&JsonValue::String("1122".to_string())));
        assert!(obj.get("observed_timestamp").is_some());
    }
}
