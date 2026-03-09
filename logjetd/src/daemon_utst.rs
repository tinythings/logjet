use super::{
    BatchPriority, ConnectionLimiter, IngestDecision, SharedIngestPolicy,
    classify_otlp_batch_priority, read_http_request, write_http_response,
};
use crate::config::{IngestOverloadConfig, SeverityFloor};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::resource::v1::Resource;
use std::io::Cursor;
use std::sync::Arc;

#[test]
fn read_http_request_parses_valid_request() {
    let bytes = b"POST /v1/logs HTTP/1.1\r\nHost: example\r\nContent-Length: 3\r\n\r\nabc";
    let mut cursor = Cursor::new(bytes.as_slice());
    let request = read_http_request(&mut cursor, 1024).unwrap();
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/v1/logs");
    assert_eq!(request.body, b"abc");
}

#[test]
fn read_http_request_rejects_missing_content_length() {
    let bytes = b"POST /v1/logs HTTP/1.1\r\nHost: example\r\n\r\nabc";
    let mut cursor = Cursor::new(bytes.as_slice());
    let err = read_http_request(&mut cursor, 1024).err().unwrap();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn read_http_request_rejects_short_body() {
    let bytes = b"POST /v1/logs HTTP/1.1\r\nHost: example\r\nContent-Length: 5\r\n\r\nabc";
    let mut cursor = Cursor::new(bytes.as_slice());
    let err = read_http_request(&mut cursor, 1024).err().unwrap();
    assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
}

#[test]
fn read_http_request_rejects_invalid_request_line() {
    let bytes = b"POST\r\nHost: example\r\nContent-Length: 0\r\n\r\n";
    let mut cursor = Cursor::new(bytes.as_slice());
    let err = read_http_request(&mut cursor, 1024).err().unwrap();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn read_http_request_rejects_payloads_over_limit() {
    let bytes = b"POST /v1/logs HTTP/1.1\r\nHost: example\r\nContent-Length: 6\r\n\r\nabcdef";
    let mut cursor = Cursor::new(bytes.as_slice());
    let err = read_http_request(&mut cursor, 5).err().unwrap();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(err.to_string(), "payload too large");
}

#[test]
fn write_http_response_writes_status_line() {
    let mut bytes = Vec::new();
    write_http_response(&mut bytes, 404, "not found").unwrap();
    let response = String::from_utf8(bytes).unwrap();
    assert!(response.starts_with("HTTP/1.1 404 Not Found\r\n"));
    assert!(response.ends_with("not found"));
}

#[test]
fn write_http_response_supports_payload_too_large_status() {
    let mut bytes = Vec::new();
    write_http_response(&mut bytes, 413, "payload too large").unwrap();
    let response = String::from_utf8(bytes).unwrap();
    assert!(response.starts_with("HTTP/1.1 413 Payload Too Large\r\n"));
    assert!(response.ends_with("payload too large"));
}

#[test]
fn write_http_response_supports_service_unavailable_status() {
    let mut bytes = Vec::new();
    write_http_response(&mut bytes, 503, "busy").unwrap();
    let response = String::from_utf8(bytes).unwrap();
    assert!(response.starts_with("HTTP/1.1 503 Service Unavailable\r\n"));
    assert!(response.ends_with("busy"));
}

#[test]
fn write_http_response_supports_too_many_requests_status() {
    let mut bytes = Vec::new();
    write_http_response(&mut bytes, 429, "rate limit exceeded").unwrap();
    let response = String::from_utf8(bytes).unwrap();
    assert!(response.starts_with("HTTP/1.1 429 Too Many Requests\r\n"));
    assert!(response.ends_with("rate limit exceeded"));
}

#[test]
fn connection_limiter_rejects_when_max_clients_is_reached() {
    let limiter = Arc::new(ConnectionLimiter::new(1));
    let first = limiter.try_acquire();
    let second = limiter.try_acquire();

    assert!(first.is_some());
    assert!(second.is_none());
}

#[test]
fn connection_limiter_accepts_again_after_permit_is_dropped() {
    let limiter = Arc::new(ConnectionLimiter::new(1));
    let first = limiter.try_acquire();
    assert!(first.is_some());
    drop(first);

    let second = limiter.try_acquire();
    assert!(second.is_some());
}

#[test]
fn ingest_policy_rate_limits_low_priority_batches() {
    let policy = SharedIngestPolicy::new(IngestOverloadConfig {
        max_batches_per_second: 1,
        priority_severity_floor: SeverityFloor::Error,
        report_every_ms: 0,
    });
    assert_eq!(
        policy.decide(BatchPriority::Warn).unwrap(),
        IngestDecision::Accept
    );
    assert_eq!(
        policy.decide(BatchPriority::Warn).unwrap(),
        IngestDecision::RejectRateLimited
    );
}

#[test]
fn ingest_policy_allows_priority_bypass_above_threshold() {
    let policy = SharedIngestPolicy::new(IngestOverloadConfig {
        max_batches_per_second: 1,
        priority_severity_floor: SeverityFloor::Error,
        report_every_ms: 0,
    });
    assert_eq!(
        policy.decide(BatchPriority::Warn).unwrap(),
        IngestDecision::Accept
    );
    assert_eq!(
        policy.decide(BatchPriority::Error).unwrap(),
        IngestDecision::AcceptPriorityBypass
    );
}

#[test]
fn classify_otlp_batch_priority_uses_highest_log_severity() {
    let batch = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: Vec::new(),
                dropped_attributes_count: 0,
            }),
            scope_logs: vec![ScopeLogs {
                scope: Some(InstrumentationScope {
                    name: "test".to_string(),
                    version: String::new(),
                    attributes: Vec::new(),
                    dropped_attributes_count: 0,
                }),
                log_records: vec![
                    LogRecord {
                        severity_number: 13,
                        severity_text: "WARN".to_string(),
                        body: Some(AnyValue {
                            value: Some(opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(
                                "warn".to_string(),
                            )),
                        }),
                        ..Default::default()
                    },
                    LogRecord {
                        severity_number: 17,
                        severity_text: "ERROR".to_string(),
                        body: Some(AnyValue {
                            value: Some(opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(
                                "error".to_string(),
                            )),
                        }),
                        ..Default::default()
                    },
                ],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };

    assert_eq!(classify_otlp_batch_priority(&batch), BatchPriority::Error);
}
