use logjet::{OwnedRecord, RecordType};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope, KeyValue};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::metrics::v1::number_data_point::Value as DataPointValue;
use opentelemetry_proto::tonic::metrics::v1::{Gauge, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
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

#[test]
fn export_ndjson_objects_includes_otlp_metrics_fields() {
    let metric = Metric {
        name: "cpu.usage".to_string(),
        description: "CPU usage".to_string(),
        unit: "%".to_string(),
        data: Some(opentelemetry_proto::tonic::metrics::v1::metric::Data::Gauge(Gauge {
            data_points: vec![NumberDataPoint {
                attributes: vec![KeyValue { key: "cpu".to_string(), value: Some(AnyValue { value: Some(Value::StringValue("all".to_string())) }) }],
                start_time_unix_nano: 0,
                time_unix_nano: 1_700_000_000_000_000_000,
                value: Some(DataPointValue::AsDouble(45.5)),
                flags: 0,
                exemplars: vec![],
            }],
        })),
        metadata: vec![],
    };
    let batch = ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            resource: Some(Resource {
                attributes: vec![KeyValue {
                    key: "service.name".to_string(),
                    value: Some(AnyValue { value: Some(Value::StringValue("metrics-svc".to_string())) }),
                }],
                dropped_attributes_count: 0,
                entity_refs: vec![],
            }),
            scope_metrics: vec![ScopeMetrics {
                scope: Some(InstrumentationScope {
                    name: "demo.metrics.scope".to_string(),
                    version: "1.0.0".to_string(),
                    attributes: vec![],
                    dropped_attributes_count: 0,
                }),
                metrics: vec![metric],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };
    let record = OwnedRecord { record_type: RecordType::Metrics, seq: 1, ts_unix_ns: 1_700_000_000_000_000_000, payload: batch.encode_to_vec() };

    let docs = export_ndjson_objects(&record, &[]);
    assert_eq!(docs.len(), 1);
    let Some(obj) = docs.first().and_then(JsonValue::as_object) else { panic!("expected object") };
    assert_eq!(obj.get("metric_name"), Some(&JsonValue::String("cpu.usage".to_string())));
    assert_eq!(obj.get("metric_type"), Some(&JsonValue::String("Gauge".to_string())));
    assert_eq!(obj.get("metric_unit"), Some(&JsonValue::String("%".to_string())));
    assert_eq!(obj.get("metric_description"), Some(&JsonValue::String("CPU usage".to_string())));
    assert_eq!(obj.get("value"), Some(&JsonValue::Number(serde_json::Number::from_f64(45.5).unwrap())));
    assert_eq!(obj.get("service_name"), Some(&JsonValue::String("metrics-svc".to_string())));
    assert_eq!(obj.get("cpu"), Some(&JsonValue::String("all".to_string())));
    assert!(obj.get("timestamp").is_some());
}

#[test]
fn export_ndjson_objects_includes_otlp_traces_fields() {
    let batch = ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(Resource {
                attributes: vec![KeyValue {
                    key: "service.name".to_string(),
                    value: Some(AnyValue { value: Some(Value::StringValue("traces-svc".to_string())) }),
                }],
                dropped_attributes_count: 0,
                entity_refs: vec![],
            }),
            scope_spans: vec![ScopeSpans {
                scope: Some(InstrumentationScope {
                    name: "demo.traces.scope".to_string(),
                    version: "2.0.0".to_string(),
                    attributes: vec![],
                    dropped_attributes_count: 0,
                }),
                spans: vec![Span {
                    trace_id: vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
                    span_id: vec![16, 17, 18, 19, 20, 21, 22, 23],
                    parent_span_id: vec![],
                    name: "GET /api".to_string(),
                    kind: 2,
                    start_time_unix_nano: 1_700_000_000_000_000_000,
                    end_time_unix_nano: 1_700_000_000_000_000_100,
                    attributes: vec![KeyValue {
                        key: "http.method".to_string(),
                        value: Some(AnyValue { value: Some(Value::StringValue("GET".to_string())) }),
                    }],
                    dropped_attributes_count: 0,
                    events: vec![],
                    dropped_events_count: 0,
                    links: vec![],
                    dropped_links_count: 0,
                    status: Some(opentelemetry_proto::tonic::trace::v1::Status { code: 1, message: String::new() }),
                    flags: 0,
                    trace_state: String::new(),
                }],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };
    let record = OwnedRecord { record_type: RecordType::Traces, seq: 1, ts_unix_ns: 1_700_000_000_000_000_000, payload: batch.encode_to_vec() };

    let docs = export_ndjson_objects(&record, &[]);
    assert_eq!(docs.len(), 1);
    let Some(obj) = docs.first().and_then(JsonValue::as_object) else { panic!("expected object") };
    assert_eq!(obj.get("name"), Some(&JsonValue::String("GET /api".to_string())));
    assert_eq!(obj.get("kind"), Some(&JsonValue::String("Server".to_string())));
    assert_eq!(obj.get("trace_id"), Some(&JsonValue::String("0102030405060708090a0b0c0d0e0f10".to_string())));
    assert_eq!(obj.get("span_id"), Some(&JsonValue::String("1011121314151617".to_string())));
    assert_eq!(obj.get("status_code"), Some(&JsonValue::Number(1.into())));
    assert_eq!(obj.get("duration_ns"), Some(&JsonValue::Number(100.into())));
    assert_eq!(obj.get("service_name"), Some(&JsonValue::String("traces-svc".to_string())));
    assert_eq!(obj.get("scope_name"), Some(&JsonValue::String("demo.traces.scope".to_string())));
    assert_eq!(obj.get("http_method"), Some(&JsonValue::String("GET".to_string())));
    assert!(obj.get("start_time").is_some());
    assert!(obj.get("end_time").is_some());
}
