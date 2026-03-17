use super::{
    DetailRecord, EntryMeta, MODAL_ATTR_ENTRY_LIMIT_PER_KIND, extract_otlp_log_message, format_summary, render_modal_info_entries,
    render_modal_message, text_preview,
};
use logjet::RecordType;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope, KeyValue};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message;

#[test]
fn text_preview_flattens_newlines() {
    assert_eq!(text_preview(b"hello\nworld", 32), "hello world");
}

#[test]
fn summary_uses_trimmed_single_line_preview() {
    let detail = DetailRecord {
        meta: EntryMeta { offset: 0, record_type: RecordType::Logs, seq: 7, ts_unix_ns: 9, payload_len: 13 },
        payload: b"line one\nline two".to_vec(),
    };
    let summary = format_summary(&detail, false);
    assert_eq!(summary, "line one line two");
}

#[test]
fn summary_prefers_decoded_otlp_log_message() {
    let batch = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource { attributes: Vec::new(), dropped_attributes_count: 0 }),
            scope_logs: vec![ScopeLogs {
                scope: Some(InstrumentationScope {
                    name: "test".to_string(),
                    version: String::new(),
                    attributes: Vec::new(),
                    dropped_attributes_count: 0,
                }),
                log_records: vec![LogRecord {
                    time_unix_nano: 0,
                    observed_time_unix_nano: 0,
                    severity_number: 0,
                    severity_text: String::new(),
                    body: Some(AnyValue { value: Some(Value::StringValue("hello from body".to_string())) }),
                    attributes: Vec::new(),
                    dropped_attributes_count: 0,
                    flags: 0,
                    trace_id: Vec::new(),
                    span_id: Vec::new(),
                    event_name: String::new(),
                }],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };
    let payload = batch.encode_to_vec();
    let detail = DetailRecord {
        meta: EntryMeta { offset: 0, record_type: RecordType::Logs, seq: 1, ts_unix_ns: 2, payload_len: payload.len() as u64 },
        payload,
    };

    assert_eq!(extract_otlp_log_message(&detail.payload).as_deref(), Some("hello from body"));
    assert_eq!(format_summary(&detail, false), "hello from body");
}

#[test]
fn modal_falls_back_to_raw_payload() {
    let detail = DetailRecord {
        meta: EntryMeta { offset: 0, record_type: RecordType::Metrics, seq: 1, ts_unix_ns: 2, payload_len: 5 },
        payload: b"hello".to_vec(),
    };
    let body = render_modal_message(&detail, false);
    assert_eq!(body, "hello");
}

#[test]
fn modal_info_lists_otlp_attributes() {
    let batch = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: vec![KeyValue {
                    key: "service.name".to_string(),
                    value: Some(AnyValue { value: Some(Value::StringValue("cpp-appliance".to_string())) }),
                }],
                dropped_attributes_count: 0,
            }),
            scope_logs: vec![ScopeLogs {
                scope: Some(InstrumentationScope {
                    name: "liblogjet".to_string(),
                    version: String::new(),
                    attributes: Vec::new(),
                    dropped_attributes_count: 0,
                }),
                log_records: vec![LogRecord {
                    time_unix_nano: 0,
                    observed_time_unix_nano: 0,
                    severity_number: 0,
                    severity_text: "INFO".to_string(),
                    body: Some(AnyValue { value: Some(Value::StringValue("hello from cpp".to_string())) }),
                    attributes: vec![KeyValue {
                        key: "character".to_string(),
                        value: Some(AnyValue { value: Some(Value::StringValue("Bender".to_string())) }),
                    }],
                    dropped_attributes_count: 0,
                    flags: 0,
                    trace_id: Vec::new(),
                    span_id: Vec::new(),
                    event_name: String::new(),
                }],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };
    let payload = batch.encode_to_vec();
    let detail = DetailRecord {
        meta: EntryMeta { offset: 0, record_type: RecordType::Logs, seq: 1, ts_unix_ns: 2, payload_len: payload.len() as u64 },
        payload,
    };

    let entries = render_modal_info_entries(&detail);
    assert!(entries.iter().any(|(key, value)| key == "resource.service.name" && value == "cpp-appliance"));
    assert!(entries.iter().any(|(key, value)| key == "record.character" && value == "Bender"));
}

#[test]
fn modal_info_caps_attribute_entries_per_kind() {
    let attributes = (0..40)
        .map(|index| KeyValue { key: format!("custom.{index}"), value: Some(AnyValue { value: Some(Value::StringValue(format!("value-{index}"))) }) })
        .collect::<Vec<_>>();
    let batch = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource { attributes: Vec::new(), dropped_attributes_count: 0 }),
            scope_logs: vec![ScopeLogs {
                scope: Some(InstrumentationScope {
                    name: "liblogjet".to_string(),
                    version: String::new(),
                    attributes: Vec::new(),
                    dropped_attributes_count: 0,
                }),
                log_records: vec![LogRecord {
                    time_unix_nano: 0,
                    observed_time_unix_nano: 0,
                    severity_number: 0,
                    severity_text: "INFO".to_string(),
                    body: Some(AnyValue { value: Some(Value::StringValue("hello from cpp".to_string())) }),
                    attributes,
                    dropped_attributes_count: 0,
                    flags: 0,
                    trace_id: Vec::new(),
                    span_id: Vec::new(),
                    event_name: String::new(),
                }],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };
    let payload = batch.encode_to_vec();
    let detail = DetailRecord {
        meta: EntryMeta { offset: 0, record_type: RecordType::Logs, seq: 1, ts_unix_ns: 2, payload_len: payload.len() as u64 },
        payload,
    };

    let entries = render_modal_info_entries(&detail);
    let record_entries = entries.iter().filter(|(key, _)| key.starts_with("record.custom.")).count();
    assert_eq!(record_entries, MODAL_ATTR_ENTRY_LIMIT_PER_KIND);
    assert!(entries.iter().any(|(key, value)| key == "record.attrs.more" && value == "8 not shown"));
}
