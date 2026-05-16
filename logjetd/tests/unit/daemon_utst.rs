use super::{
    BatchPriority, ConnectionLimiter, IngestDecision, SharedIngestPolicy, classify_otlp_batch_priority, extract_batch_timestamp_metrics,
    extract_batch_timestamp_traces, handle_otlp_http_request, maybe_decompress_body,
};
use crate::config::{IngestOverloadConfig, SeverityFloor};
use http_body_util::Full;
use hyper::body::Bytes;
use hyper::{Method, Request, StatusCode};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::metrics::v1::number_data_point::Value as DataPointValue;
use opentelemetry_proto::tonic::metrics::v1::{Gauge, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
use prost::Message;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

#[tokio::test]
async fn handle_otlp_http_request_logs_accepts_valid_batch() {
    let spool = crate::spool::Spool::open(crate::config::StorageConfig::Buffer(crate::config::BufferConfig {
        limit: crate::config::BufferLimit::Bytes(1024 * 1024),
        keep_messages: 0,
    }))
    .unwrap();
    let shared_spool = Arc::new(super::SharedSpool::new(spool));
    let policy = Arc::new(SharedIngestPolicy::new(IngestOverloadConfig {
        max_batches_per_second: 0,
        priority_severity_floor: SeverityFloor::Error,
        report_every_ms: 0,
    }));
    let next_seq = Arc::new(AtomicU64::new(1));

    let batch = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource { attributes: vec![], dropped_attributes_count: 0, entity_refs: vec![] }),
            scope_logs: vec![ScopeLogs {
                scope: Some(InstrumentationScope {
                    name: "test".to_string(),
                    version: String::new(),
                    attributes: vec![],
                    dropped_attributes_count: 0,
                }),
                log_records: vec![LogRecord {
                    severity_number: 13,
                    severity_text: "WARN".to_string(),
                    body: Some(AnyValue {
                        value: Some(opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue("warn".to_string())),
                    }),
                    ..Default::default()
                }],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };
    let body = batch.encode_to_vec();
    let req = Request::builder()
        .method(Method::POST)
        .uri("/v1/logs")
        .header("content-length", body.len().to_string())
        .body(Full::new(Bytes::from(body)))
        .unwrap();

    let response = handle_otlp_http_request(req, shared_spool, policy, next_seq, 1024 * 1024).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn handle_otlp_http_request_metrics_accepts_valid_batch() {
    let spool = crate::spool::Spool::open(crate::config::StorageConfig::Buffer(crate::config::BufferConfig {
        limit: crate::config::BufferLimit::Bytes(1024 * 1024),
        keep_messages: 0,
    }))
    .unwrap();
    let shared_spool = Arc::new(super::SharedSpool::new(spool));
    let policy = Arc::new(SharedIngestPolicy::new(IngestOverloadConfig {
        max_batches_per_second: 0,
        priority_severity_floor: SeverityFloor::Error,
        report_every_ms: 0,
    }));
    let next_seq = Arc::new(AtomicU64::new(1));

    let batch = ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            resource: Some(Resource { attributes: vec![], dropped_attributes_count: 0, entity_refs: vec![] }),
            scope_metrics: vec![ScopeMetrics {
                scope: Some(InstrumentationScope {
                    name: "test".to_string(),
                    version: String::new(),
                    attributes: vec![],
                    dropped_attributes_count: 0,
                }),
                metrics: vec![Metric {
                    name: "cpu".to_string(),
                    description: String::new(),
                    unit: "%".to_string(),
                    data: Some(opentelemetry_proto::tonic::metrics::v1::metric::Data::Gauge(Gauge {
                        data_points: vec![NumberDataPoint {
                            attributes: vec![],
                            start_time_unix_nano: 0,
                            time_unix_nano: 1_700_000_000_000_000_000,
                            value: Some(DataPointValue::AsDouble(42.0)),
                            flags: 0,
                            exemplars: vec![],
                        }],
                    })),
                    metadata: vec![],
                }],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };
    let body = batch.encode_to_vec();
    let req = Request::builder()
        .method(Method::POST)
        .uri("/v1/metrics")
        .header("content-length", body.len().to_string())
        .body(Full::new(Bytes::from(body)))
        .unwrap();

    let response = handle_otlp_http_request(req, shared_spool, policy, next_seq, 1024 * 1024).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn handle_otlp_http_request_traces_accepts_valid_batch() {
    let spool = crate::spool::Spool::open(crate::config::StorageConfig::Buffer(crate::config::BufferConfig {
        limit: crate::config::BufferLimit::Bytes(1024 * 1024),
        keep_messages: 0,
    }))
    .unwrap();
    let shared_spool = Arc::new(super::SharedSpool::new(spool));
    let policy = Arc::new(SharedIngestPolicy::new(IngestOverloadConfig {
        max_batches_per_second: 0,
        priority_severity_floor: SeverityFloor::Error,
        report_every_ms: 0,
    }));
    let next_seq = Arc::new(AtomicU64::new(1));

    let batch = ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(Resource { attributes: vec![], dropped_attributes_count: 0, entity_refs: vec![] }),
            scope_spans: vec![ScopeSpans {
                scope: Some(InstrumentationScope {
                    name: "test".to_string(),
                    version: String::new(),
                    attributes: vec![],
                    dropped_attributes_count: 0,
                }),
                spans: vec![Span {
                    trace_id: vec![1, 2, 3, 4],
                    span_id: vec![5, 6, 7, 8],
                    parent_span_id: vec![],
                    name: "test-span".to_string(),
                    kind: 1,
                    start_time_unix_nano: 1_700_000_000_000_000_000,
                    end_time_unix_nano: 1_700_000_000_000_000_001,
                    attributes: vec![],
                    dropped_attributes_count: 0,
                    events: vec![],
                    dropped_events_count: 0,
                    links: vec![],
                    dropped_links_count: 0,
                    status: None,
                    flags: 0,
                    trace_state: String::new(),
                }],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };
    let body = batch.encode_to_vec();
    let req = Request::builder()
        .method(Method::POST)
        .uri("/v1/traces")
        .header("content-length", body.len().to_string())
        .body(Full::new(Bytes::from(body)))
        .unwrap();

    let response = handle_otlp_http_request(req, shared_spool, policy, next_seq, 1024 * 1024).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn handle_otlp_http_request_rejects_non_post() {
    let spool = crate::spool::Spool::open(crate::config::StorageConfig::Buffer(crate::config::BufferConfig {
        limit: crate::config::BufferLimit::Bytes(1024),
        keep_messages: 0,
    }))
    .unwrap();
    let shared_spool = Arc::new(super::SharedSpool::new(spool));
    let policy = Arc::new(SharedIngestPolicy::new(IngestOverloadConfig {
        max_batches_per_second: 0,
        priority_severity_floor: SeverityFloor::Error,
        report_every_ms: 0,
    }));
    let next_seq = Arc::new(AtomicU64::new(1));

    let req = Request::builder().method(Method::GET).uri("/v1/logs").body(Full::new(Bytes::new())).unwrap();
    let response = handle_otlp_http_request(req, shared_spool, policy, next_seq, 1024).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn handle_otlp_http_request_unknown_path_returns_404() {
    let spool = crate::spool::Spool::open(crate::config::StorageConfig::Buffer(crate::config::BufferConfig {
        limit: crate::config::BufferLimit::Bytes(1024),
        keep_messages: 0,
    }))
    .unwrap();
    let shared_spool = Arc::new(super::SharedSpool::new(spool));
    let policy = Arc::new(SharedIngestPolicy::new(IngestOverloadConfig {
        max_batches_per_second: 0,
        priority_severity_floor: SeverityFloor::Error,
        report_every_ms: 0,
    }));
    let next_seq = Arc::new(AtomicU64::new(1));

    let req = Request::builder()
        .method(Method::POST)
        .uri("/v1/unknown")
        .body(Full::new(Bytes::new()))
        .unwrap();
    let response = handle_otlp_http_request(req, shared_spool, policy, next_seq, 1024).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn handle_otlp_http_request_rejects_body_over_limit() {
    let spool = crate::spool::Spool::open(crate::config::StorageConfig::Buffer(crate::config::BufferConfig {
        limit: crate::config::BufferLimit::Bytes(1024),
        keep_messages: 0,
    }))
    .unwrap();
    let shared_spool = Arc::new(super::SharedSpool::new(spool));
    let policy = Arc::new(SharedIngestPolicy::new(IngestOverloadConfig {
        max_batches_per_second: 0,
        priority_severity_floor: SeverityFloor::Error,
        report_every_ms: 0,
    }));
    let next_seq = Arc::new(AtomicU64::new(1));

    let body = vec![0u8; 100];
    let req = Request::builder()
        .method(Method::POST)
        .uri("/v1/logs")
        .header("content-length", body.len().to_string())
        .body(Full::new(Bytes::from(body)))
        .unwrap();

    let response = handle_otlp_http_request(req, shared_spool, policy, next_seq, 50).await.unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn handle_otlp_http_request_rejects_invalid_protobuf() {
    let spool = crate::spool::Spool::open(crate::config::StorageConfig::Buffer(crate::config::BufferConfig {
        limit: crate::config::BufferLimit::Bytes(1024),
        keep_messages: 0,
    }))
    .unwrap();
    let shared_spool = Arc::new(super::SharedSpool::new(spool));
    let policy = Arc::new(SharedIngestPolicy::new(IngestOverloadConfig {
        max_batches_per_second: 0,
        priority_severity_floor: SeverityFloor::Error,
        report_every_ms: 0,
    }));
    let next_seq = Arc::new(AtomicU64::new(1));

    let body = b"not valid protobuf";
    let req = Request::builder()
        .method(Method::POST)
        .uri("/v1/logs")
        .header("content-length", body.len().to_string())
        .body(Full::new(Bytes::from(body.to_vec())))
        .unwrap();

    let response = handle_otlp_http_request(req, shared_spool, policy, next_seq, 1024).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn handle_otlp_http_request_rate_limits_when_overloaded() {
    let spool = crate::spool::Spool::open(crate::config::StorageConfig::Buffer(crate::config::BufferConfig {
        limit: crate::config::BufferLimit::Bytes(1024 * 1024),
        keep_messages: 0,
    }))
    .unwrap();
    let shared_spool = Arc::new(super::SharedSpool::new(spool));
    let policy = Arc::new(SharedIngestPolicy::new(IngestOverloadConfig {
        max_batches_per_second: 1,
        priority_severity_floor: SeverityFloor::Error,
        report_every_ms: 0,
    }));
    let next_seq = Arc::new(AtomicU64::new(1));

    let batch = ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            resource: Some(Resource { attributes: vec![], dropped_attributes_count: 0, entity_refs: vec![] }),
            scope_metrics: vec![ScopeMetrics {
                scope: Some(InstrumentationScope {
                    name: "test".to_string(),
                    version: String::new(),
                    attributes: vec![],
                    dropped_attributes_count: 0,
                }),
                metrics: vec![],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };
    let body = batch.encode_to_vec();

    let req1 = Request::builder()
        .method(Method::POST)
        .uri("/v1/metrics")
        .header("content-length", body.len().to_string())
        .body(Full::new(Bytes::from(body.clone())))
        .unwrap();
    let response1 = handle_otlp_http_request(req1, Arc::clone(&shared_spool), Arc::clone(&policy), Arc::clone(&next_seq), 1024 * 1024).await.unwrap();
    assert_eq!(response1.status(), StatusCode::OK);

    let req2 = Request::builder()
        .method(Method::POST)
        .uri("/v1/metrics")
        .header("content-length", body.len().to_string())
        .body(Full::new(Bytes::from(body)))
        .unwrap();
    let response2 = handle_otlp_http_request(req2, shared_spool, policy, next_seq, 1024 * 1024).await.unwrap();
    assert_eq!(response2.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[test]
fn maybe_decompress_body_passes_through_uncompressed() {
    let data = b"hello";
    let out = maybe_decompress_body(data.to_vec(), None).unwrap();
    assert_eq!(out, data);
}

#[test]
fn maybe_decompress_body_decompresses_gzip() {
    let data = b"hello world";
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(data).unwrap();
    let compressed = encoder.finish().unwrap();
    let out = maybe_decompress_body(compressed, Some("gzip")).unwrap();
    assert_eq!(out, data);
}

#[test]
fn maybe_decompress_body_decompresses_x_gzip() {
    let data = b"hello world";
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(data).unwrap();
    let compressed = encoder.finish().unwrap();
    let out = maybe_decompress_body(compressed, Some("x-gzip")).unwrap();
    assert_eq!(out, data);
}

#[test]
fn maybe_decompress_body_rejects_invalid_gzip() {
    let err = maybe_decompress_body(b"not-gzip".to_vec(), Some("gzip")).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
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
    assert_eq!(policy.decide(BatchPriority::Warn).unwrap(), IngestDecision::Accept);
    assert_eq!(policy.decide(BatchPriority::Warn).unwrap(), IngestDecision::RejectRateLimited);
}

#[test]
fn ingest_policy_allows_priority_bypass_above_threshold() {
    let policy = SharedIngestPolicy::new(IngestOverloadConfig {
        max_batches_per_second: 1,
        priority_severity_floor: SeverityFloor::Error,
        report_every_ms: 0,
    });
    assert_eq!(policy.decide(BatchPriority::Warn).unwrap(), IngestDecision::Accept);
    assert_eq!(policy.decide(BatchPriority::Error).unwrap(), IngestDecision::AcceptPriorityBypass);
}

#[test]
fn classify_otlp_batch_priority_uses_highest_log_severity() {
    let batch = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource { attributes: Vec::new(), dropped_attributes_count: 0, entity_refs: Vec::new() }),
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
                            value: Some(opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue("warn".to_string())),
                        }),
                        ..Default::default()
                    },
                    LogRecord {
                        severity_number: 17,
                        severity_text: "ERROR".to_string(),
                        body: Some(AnyValue {
                            value: Some(opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue("error".to_string())),
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

#[test]
fn extract_batch_timestamp_metrics_finds_first_datapoint_time() {
    let metric = Metric {
        name: "cpu".to_string(),
        description: String::new(),
        unit: "%".to_string(),
        data: Some(opentelemetry_proto::tonic::metrics::v1::metric::Data::Gauge(Gauge {
            data_points: vec![NumberDataPoint {
                attributes: vec![],
                start_time_unix_nano: 0,
                time_unix_nano: 1_700_000_000_000_000_000,
                value: Some(DataPointValue::AsDouble(42.0)),
                flags: 0,
                exemplars: vec![],
            }],
        })),
        metadata: vec![],
    };
    let batch = ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            resource: Some(Resource { attributes: vec![], dropped_attributes_count: 0, entity_refs: vec![] }),
            scope_metrics: vec![ScopeMetrics {
                scope: Some(InstrumentationScope { name: "test".to_string(), version: String::new(), attributes: vec![], dropped_attributes_count: 0 }),
                metrics: vec![metric],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };
    assert_eq!(extract_batch_timestamp_metrics(&batch), Some(1_700_000_000_000_000_000));
}

#[test]
fn extract_batch_timestamp_metrics_returns_none_when_empty() {
    let batch = ExportMetricsServiceRequest { resource_metrics: vec![] };
    assert_eq!(extract_batch_timestamp_metrics(&batch), None);
}

#[test]
fn extract_batch_timestamp_traces_finds_first_span_start_time() {
    let batch = ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(Resource { attributes: vec![], dropped_attributes_count: 0, entity_refs: vec![] }),
            scope_spans: vec![ScopeSpans {
                scope: Some(InstrumentationScope { name: "test".to_string(), version: String::new(), attributes: vec![], dropped_attributes_count: 0 }),
                spans: vec![Span {
                    trace_id: vec![1, 2, 3, 4],
                    span_id: vec![5, 6, 7, 8],
                    parent_span_id: vec![],
                    name: "test-span".to_string(),
                    kind: 1,
                    start_time_unix_nano: 1_700_000_000_000_000_000,
                    end_time_unix_nano: 1_700_000_000_000_000_001,
                    attributes: vec![],
                    dropped_attributes_count: 0,
                    events: vec![],
                    dropped_events_count: 0,
                    links: vec![],
                    dropped_links_count: 0,
                    status: None,
                    flags: 0,
                    trace_state: String::new(),
                }],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };
    assert_eq!(extract_batch_timestamp_traces(&batch), Some(1_700_000_000_000_000_000));
}

#[test]
fn extract_batch_timestamp_traces_returns_none_when_empty() {
    let batch = ExportTraceServiceRequest { resource_spans: vec![] };
    assert_eq!(extract_batch_timestamp_traces(&batch), None);
}
