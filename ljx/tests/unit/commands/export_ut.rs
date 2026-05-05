use logjet::{OwnedRecord, RecordType};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope, KeyValue};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message;
use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::commands::export::{export_ndjson_objects, export_ndjson_objects_with_preview, select_json_fields};

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

#[test]
fn export_ndjson_objects_with_preview_truncates_long_fields() {
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
                    body: Some(AnyValue { value: Some(Value::StringValue("hello-world".to_string())) }),
                    attributes: vec![KeyValue {
                        key: "http.target".to_string(),
                        value: Some(AnyValue { value: Some(Value::StringValue("/api/v1/very/long/path".to_string())) }),
                    }],
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

    let docs = export_ndjson_objects_with_preview(&record, &[], Some(6));
    let Some(obj) = docs.first().and_then(JsonValue::as_object) else { panic!("expected object") };
    assert_eq!(obj.get("body"), Some(&JsonValue::String("hello-...".to_string())));
    assert_eq!(obj.get("http_target"), Some(&JsonValue::String("/api/v...".to_string())));
}
