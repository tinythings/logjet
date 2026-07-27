use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope, KeyValue};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::metrics::v1::{Gauge, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span, Status};
use logjet::RecordType;
use prost::Message;

use crate::CelExpression;

fn make_log_payload(records: Vec<LogRecord>) -> Vec<u8> {
    make_log_payload_with_context(records, &[], &[], "test-svc")
}

fn make_log_payload_with_context(
    records: Vec<LogRecord>,
    resource_attrs: &[KeyValue],
    scope_attrs: &[KeyValue],
    service_name: &str,
) -> Vec<u8> {
    let mut res_attrs = vec![KeyValue {
        key: "service.name".to_string(),
        value: Some(AnyValue { value: Some(Value::StringValue(service_name.to_string())) }),
    }];
    res_attrs.extend_from_slice(resource_attrs);

    let batch = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource { attributes: res_attrs, dropped_attributes_count: 0, entity_refs: Vec::new() }),
            scope_logs: vec![ScopeLogs {
                scope: Some(InstrumentationScope {
                    name: "test-scope".to_string(),
                    version: "1.0.0".to_string(),
                    attributes: scope_attrs.to_vec(),
                    dropped_attributes_count: 0,
                }),
                log_records: records,
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };
    batch.encode_to_vec()
}

fn string_attr(key: &str, value: &str) -> KeyValue {
    KeyValue { key: key.to_string(), value: Some(AnyValue { value: Some(Value::StringValue(value.to_string())) }), ..Default::default() }
}

fn int_attr(key: &str, value: i64) -> KeyValue {
    KeyValue { key: key.to_string(), value: Some(AnyValue { value: Some(Value::IntValue(value)) }), ..Default::default() }
}

fn log_record(body: &str, severity_number: i32, severity_text: &str) -> LogRecord {
    LogRecord {
        body: Some(AnyValue { value: Some(Value::StringValue(body.to_string())) }),
        severity_number,
        severity_text: severity_text.to_string(),
        ..Default::default()
    }
}

fn log_record_with_attrs(body: &str, severity_number: i32, severity_text: &str, attrs: &[KeyValue]) -> LogRecord {
    LogRecord {
        body: Some(AnyValue { value: Some(Value::StringValue(body.to_string())) }),
        severity_number,
        severity_text: severity_text.to_string(),
        attributes: attrs.to_vec(),
        ..Default::default()
    }
}

#[test]
fn compile_simple_comparison() {
    let expr = CelExpression::compile("severity_number >= 13").unwrap();
    assert_eq!(expr.source(), "severity_number >= 13");
}

#[test]
fn compile_with_body_contains() {
    let expr = CelExpression::compile("body.contains(\"timeout\")").unwrap();
    assert_eq!(expr.source(), "body.contains(\"timeout\")");
}

#[test]
fn compile_invalid_expression_returns_error() {
    let result = CelExpression::compile("this is not valid CEL @#$%");
    assert!(result.is_err());
}

#[test]
fn severity_number_match() {
    let expr = CelExpression::compile("severity_number == 17").unwrap();
    let payload = make_log_payload(vec![log_record("msg", 17, "ERROR")]);
    assert!(expr.matches_logs_payload(&payload).unwrap());
}

#[test]
fn severity_number_no_match() {
    let expr = CelExpression::compile("severity_number == 17").unwrap();
    let payload = make_log_payload(vec![log_record("msg", 9, "INFO")]);
    assert!(!expr.matches_logs_payload(&payload).unwrap());
}

#[test]
fn severity_text_match() {
    let expr = CelExpression::compile("severity_text == \"ERROR\"").unwrap();
    let payload = make_log_payload(vec![log_record("msg", 17, "ERROR")]);
    assert!(expr.matches_logs_payload(&payload).unwrap());
}

#[test]
fn service_name_match() {
    let expr = CelExpression::compile("service_name == \"test-svc\"").unwrap();
    let payload = make_log_payload(vec![log_record("msg", 9, "INFO")]);
    assert!(expr.matches_logs_payload(&payload).unwrap());
}

#[test]
fn service_name_no_match() {
    let expr = CelExpression::compile("service_name == \"other-service\"").unwrap();
    let payload = make_log_payload(vec![log_record("msg", 9, "INFO")]);
    assert!(!expr.matches_logs_payload(&payload).unwrap());
}

#[test]
fn body_contains_match() {
    let expr = CelExpression::compile("body.contains(\"timeout\")").unwrap();
    let payload = make_log_payload(vec![log_record("connection timeout after 30s", 17, "ERROR")]);
    assert!(expr.matches_logs_payload(&payload).unwrap());
}

#[test]
fn body_contains_no_match() {
    let expr = CelExpression::compile("body.contains(\"timeout\")").unwrap();
    let payload = make_log_payload(vec![log_record("connection established", 9, "INFO")]);
    assert!(!expr.matches_logs_payload(&payload).unwrap());
}

#[test]
fn record_attribute_string_match() {
    let expr = CelExpression::compile("attributes[\"custom.thread\"] == \"worker-1\"").unwrap();
    let payload = make_log_payload(vec![log_record_with_attrs("msg", 13, "WARN", &[string_attr("custom.thread", "worker-1")])]);
    assert!(expr.matches_logs_payload(&payload).unwrap());
}

#[test]
fn record_attribute_int_match() {
    let expr = CelExpression::compile("attributes[\"custom.count\"] == 42").unwrap();
    let payload = make_log_payload(vec![log_record_with_attrs("msg", 13, "WARN", &[int_attr("custom.count", 42)])]);
    assert!(expr.matches_logs_payload(&payload).unwrap());
}

#[test]
fn resource_attribute_match() {
    let expr = CelExpression::compile("resource[\"custom.label\"].contains(\"processor-a\")").unwrap();
    let payload = make_log_payload_with_context(
        vec![log_record("msg", 9, "INFO")],
        &[string_attr("custom.label", "processor-a")],
        &[],
        "test-svc",
    );
    assert!(expr.matches_logs_payload(&payload).unwrap());
}

#[test]
fn resource_attribute_strict_equality() {
    let expr = CelExpression::compile("resource[\"custom.label\"] == \"processor-a\"").unwrap();
    let payload = make_log_payload_with_context(
        vec![log_record("msg", 9, "INFO")],
        &[string_attr("custom.label", "processor-a")],
        &[],
        "test-svc",
    );
    assert!(expr.matches_logs_payload(&payload).unwrap());
}

#[test]
fn scope_attribute_match() {
    let expr = CelExpression::compile("scope[\"custom.channel\"] >= 3").unwrap();
    let payload = make_log_payload_with_context(
        vec![log_record("msg", 9, "INFO")],
        &[],
        &[int_attr("custom.channel", 4)],
        "test-svc",
    );
    assert!(expr.matches_logs_payload(&payload).unwrap());
}

#[test]
fn combined_conditions_and() {
    let expr = CelExpression::compile("severity_number >= 17 && service_name == \"test-svc\"").unwrap();
    let payload = make_log_payload(vec![log_record("fatal error", 17, "FATAL")]);
    assert!(expr.matches_logs_payload(&payload).unwrap());
}

#[test]
fn combined_conditions_and_fails_on_one() {
    let expr = CelExpression::compile("severity_number >= 17 && service_name == \"other-service\"").unwrap();
    let payload = make_log_payload(vec![log_record("fatal error", 17, "FATAL")]);
    assert!(!expr.matches_logs_payload(&payload).unwrap());
}

#[test]
fn any_record_in_batch_matches() {
    let expr = CelExpression::compile("severity_number == 17").unwrap();
    let payload = make_log_payload(vec![
        log_record("info msg", 9, "INFO"),
        log_record("warn msg", 13, "WARN"),
        log_record("error msg", 17, "ERROR"),
    ]);
    assert!(expr.matches_logs_payload(&payload).unwrap());
}

#[test]
fn no_record_in_batch_matches() {
    let expr = CelExpression::compile("severity_number == 17").unwrap();
    let payload = make_log_payload(vec![
        log_record("info msg", 9, "INFO"),
        log_record("warn msg", 13, "WARN"),
    ]);
    assert!(!expr.matches_logs_payload(&payload).unwrap());
}

// --- metrics helpers ---

fn make_metric_payload(metric_name: &str, metric_type: &str, value: f64) -> Vec<u8> {
    let batch = ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            resource: Some(Resource {
                attributes: vec![KeyValue {
                    key: "service.name".to_string(),
                    value: Some(AnyValue { value: Some(Value::StringValue("test-svc".to_string())) }),
                    ..Default::default()
                }],
                dropped_attributes_count: 0,
                entity_refs: Vec::new(),
            }),
            scope_metrics: vec![ScopeMetrics {
                scope: Some(InstrumentationScope {
                    name: "test-scope".to_string(),
                    version: "1.0.0".to_string(),
                    attributes: vec![],
                    dropped_attributes_count: 0,
                }),
                metrics: vec![Metric {
                    name: metric_name.to_string(),
                    unit: "ms".to_string(),
                    description: String::new(),
                    metadata: vec![],
                    data: Some(match metric_type {
                        "Gauge" => opentelemetry_proto::tonic::metrics::v1::metric::Data::Gauge(Gauge {
                            data_points: vec![NumberDataPoint {
                                value: Some(
                                    opentelemetry_proto::tonic::metrics::v1::number_data_point::Value::AsDouble(value),
                                ),
                                time_unix_nano: 1000,
                                attributes: vec![],
                                start_time_unix_nano: 0,
                                flags: 0,
                                exemplars: vec![],
                            }],
                        }),
                        _ => opentelemetry_proto::tonic::metrics::v1::metric::Data::Gauge(Gauge {
                            data_points: vec![NumberDataPoint {
                                value: Some(
                                    opentelemetry_proto::tonic::metrics::v1::number_data_point::Value::AsDouble(value),
                                ),
                                time_unix_nano: 1000,
                                attributes: vec![],
                                start_time_unix_nano: 0,
                                flags: 0,
                                exemplars: vec![],
                            }],
                        }),
                    }),
                }],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };
    batch.encode_to_vec()
}

// --- traces helpers ---

fn make_trace_payload(span_name: &str, kind: i32, status_code: i32) -> Vec<u8> {
    let batch = ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(Resource {
                attributes: vec![KeyValue {
                    key: "service.name".to_string(),
                    value: Some(AnyValue { value: Some(Value::StringValue("test-svc".to_string())) }),
                    ..Default::default()
                }],
                dropped_attributes_count: 0,
                entity_refs: Vec::new(),
            }),
            scope_spans: vec![ScopeSpans {
                scope: Some(InstrumentationScope {
                    name: "test-scope".to_string(),
                    version: "1.0.0".to_string(),
                    attributes: vec![],
                    dropped_attributes_count: 0,
                }),
                spans: vec![Span {
                    trace_id: vec![1u8; 16],
                    span_id: vec![2u8; 8],
                    name: span_name.to_string(),
                    kind,
                    start_time_unix_nano: 1_000_000_000,
                    end_time_unix_nano: 2_000_000_000,
                    status: Some(Status { code: status_code, message: String::new() }),
                    ..Default::default()
                }],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };
    batch.encode_to_vec()
}

// --- metrics tests ---

#[test]
fn metric_name_match() {
    let expr = CelExpression::compile("metric_name == \"http.latency\"").unwrap();
    let payload = make_metric_payload("http.latency", "Gauge", 42.0);
    assert!(expr.matches_metrics_payload(&payload).unwrap());
}

#[test]
fn metric_name_no_match() {
    let expr = CelExpression::compile("metric_name == \"http.latency\"").unwrap();
    let payload = make_metric_payload("cpu.usage", "Gauge", 42.0);
    assert!(!expr.matches_metrics_payload(&payload).unwrap());
}

#[test]
fn metric_value_above_threshold() {
    let expr = CelExpression::compile("value >= 100.0").unwrap();
    let payload = make_metric_payload("cpu.usage", "Gauge", 150.0);
    assert!(expr.matches_metrics_payload(&payload).unwrap());
}

#[test]
fn metric_type_gauge() {
    let expr = CelExpression::compile("metric_type == \"Gauge\"").unwrap();
    let payload = make_metric_payload("cpu.usage", "Gauge", 50.0);
    assert!(expr.matches_metrics_payload(&payload).unwrap());
}

// --- traces tests ---

#[test]
fn span_name_match() {
    let expr = CelExpression::compile("name == \"GetUser\"").unwrap();
    let payload = make_trace_payload("GetUser", 2, 0);
    assert!(expr.matches_traces_payload(&payload).unwrap());
}

#[test]
fn span_kind_server() {
    let expr = CelExpression::compile("kind == 2").unwrap();
    let payload = make_trace_payload("GetUser", 2, 0);
    assert!(expr.matches_traces_payload(&payload).unwrap());
}

#[test]
fn span_status_error() {
    let expr = CelExpression::compile("status_code == 2").unwrap();
    let payload = make_trace_payload("GetUser", 2, 2);
    assert!(expr.matches_traces_payload(&payload).unwrap());
}

#[test]
fn span_duration_above_threshold() {
    let expr = CelExpression::compile("duration_ns >= 500000000").unwrap();
    let payload = make_trace_payload("GetUser", 2, 0);
    assert!(expr.matches_traces_payload(&payload).unwrap());
}

// --- generic dispatch tests ---

#[test]
fn matches_payload_dispatches_to_logs() {
    let expr = CelExpression::compile("severity_number == 17").unwrap();
    let payload = make_log_payload(vec![log_record("error msg", 17, "ERROR")]);
    assert!(expr.matches_payload(RecordType::Logs, &payload).unwrap());
}

#[test]
fn matches_payload_dispatches_to_metrics() {
    let expr = CelExpression::compile("metric_name == \"cpu.usage\"").unwrap();
    let payload = make_metric_payload("cpu.usage", "Gauge", 50.0);
    assert!(expr.matches_payload(RecordType::Metrics, &payload).unwrap());
}

#[test]
fn matches_payload_dispatches_to_traces() {
    let expr = CelExpression::compile("name == \"GetUser\"").unwrap();
    let payload = make_trace_payload("GetUser", 2, 0);
    assert!(expr.matches_payload(RecordType::Traces, &payload).unwrap());
}
