//! Integration tests for the dedup pipeline (exact mode).

use std::io::Cursor;

use logjet::{LogjetReader, LogjetWriter, RecordType};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope, KeyValue};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message;

use crate::dedup::flat_record::BucketKeyKind;
use crate::dedup::{DedupMode, DedupOpts};

fn make_log_record(body: &str, severity: i32, time_ns: u64) -> LogRecord {
    LogRecord {
        time_unix_nano: time_ns,
        observed_time_unix_nano: time_ns,
        severity_number: severity,
        severity_text: format!("SEV{severity}"),
        body: Some(AnyValue { value: Some(Value::StringValue(body.to_string())) }),
        attributes: Vec::new(),
        dropped_attributes_count: 0,
        flags: 0,
        trace_id: Vec::new(),
        span_id: Vec::new(),
        event_name: String::new(),
    }
}

fn make_batch(service: &str, records: Vec<LogRecord>) -> ExportLogsServiceRequest {
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: vec![KeyValue {
                    key: "service.name".to_string(),
                    value: Some(AnyValue { value: Some(Value::StringValue(service.to_string())) }),
                }],
                dropped_attributes_count: 0,
                entity_refs: Vec::new(),
            }),
            scope_logs: vec![ScopeLogs {
                scope: Some(InstrumentationScope {
                    name: "test".to_string(),
                    version: String::new(),
                    attributes: Vec::new(),
                    dropped_attributes_count: 0,
                }),
                log_records: records,
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    }
}

/// Write batches into a .logjet buffer and return it as bytes.
fn write_logjet(batches: &[ExportLogsServiceRequest]) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut writer = LogjetWriter::new(Cursor::new(&mut buf));
        for (i, batch) in batches.iter().enumerate() {
            let payload = batch.encode_to_vec();
            let seq = i as u64;
            let ts = 1000 + i as u64;
            writer.push(RecordType::Logs, seq, ts, &payload).unwrap();
        }
        writer.into_inner().unwrap();
    }
    buf
}

/// Run exact-mode dedup on raw logjet bytes, return output bytes.
fn run_dedup(input_bytes: &[u8], mode: DedupMode) -> Vec<u8> {
    let cursor = Cursor::new(input_bytes);
    let mut reader = LogjetReader::new(cursor);
    let unpacked = crate::dedup::unpack::unpack(&mut reader).unwrap();

    let mut out_buf = Vec::new();
    let mut writer = LogjetWriter::new(Cursor::new(&mut out_buf));
    let opts = DedupOpts { mode, bucket_key: BucketKeyKind::Default, drain: Default::default() };
    let _stats = crate::dedup::dedup(unpacked.records, unpacked.passthrough, &mut writer, &opts).unwrap();
    writer.into_inner().unwrap();
    out_buf
}

/// Decode all log groups from output bytes, returning (body, dedup.count) pairs.
fn read_groups(output_bytes: &[u8]) -> Vec<(String, i64)> {
    let cursor = Cursor::new(output_bytes);
    let mut reader = LogjetReader::new(cursor);
    let mut groups = Vec::new();
    while let Some(rec) = reader.next_record().unwrap() {
        if rec.record_type != RecordType::Logs {
            continue;
        }
        let batch = ExportLogsServiceRequest::decode(rec.payload.as_slice()).unwrap();
        for rl in &batch.resource_logs {
            for sl in &rl.scope_logs {
                for lr in &sl.log_records {
                    let body = lr
                        .body
                        .as_ref()
                        .and_then(|b| match &b.value {
                            Some(Value::StringValue(s)) => Some(s.clone()),
                            _ => None,
                        })
                        .unwrap_or_default();
                    let count = lr
                        .attributes
                        .iter()
                        .find(|a| a.key == "dedup.count")
                        .and_then(|a| match &a.value {
                            Some(AnyValue { value: Some(Value::IntValue(n)) }) => Some(*n),
                            _ => None,
                        })
                        .unwrap_or(1);
                    groups.push((body, count));
                }
            }
        }
    }
    groups
}

#[test]
fn exact_collapses_identical_bodies() {
    let batch = make_batch(
        "svc-a",
        vec![
            make_log_record("placeholder event", 9, 100),
            make_log_record("placeholder event", 9, 200),
            make_log_record("placeholder event", 9, 300),
            make_log_record("different placeholder", 9, 400),
        ],
    );
    let input = write_logjet(&[batch]);
    let output = run_dedup(&input, DedupMode::Exact);
    let groups = read_groups(&output);

    assert_eq!(groups.len(), 2);
    let hello = groups.iter().find(|(b, _)| b == "placeholder event").unwrap();
    assert_eq!(hello.1, 3);
    let diff = groups.iter().find(|(b, _)| b == "different placeholder").unwrap();
    assert_eq!(diff.1, 1);
}

#[test]
fn different_severity_stays_separate() {
    let batch1 = make_batch("svc-a", vec![make_log_record("same placeholder", 5, 100)]);
    let batch2 = make_batch("svc-a", vec![make_log_record("same placeholder", 9, 200)]);
    let input = write_logjet(&[batch1, batch2]);
    let output = run_dedup(&input, DedupMode::Exact);
    let groups = read_groups(&output);

    // Same body but different severity → different buckets → 2 groups.
    assert_eq!(groups.len(), 2);
    assert!(groups.iter().all(|(_, c)| *c == 1));
}

#[test]
fn different_service_stays_separate() {
    let batch1 = make_batch("svc-a", vec![make_log_record("same placeholder", 9, 100)]);
    let batch2 = make_batch("svc-b", vec![make_log_record("same placeholder", 9, 200)]);
    let input = write_logjet(&[batch1, batch2]);
    let output = run_dedup(&input, DedupMode::Exact);
    let groups = read_groups(&output);

    assert_eq!(groups.len(), 2);
    assert!(groups.iter().all(|(_, c)| *c == 1));
}

#[test]
fn timestamps_first_and_last_correct() {
    let batch = make_batch(
        "svc",
        vec![make_log_record("placeholder", 9, 500), make_log_record("placeholder", 9, 100), make_log_record("placeholder", 9, 900)],
    );
    let input = write_logjet(&[batch]);
    let output = run_dedup(&input, DedupMode::Exact);

    let cursor = Cursor::new(&output);
    let mut reader = LogjetReader::new(cursor);
    let rec = reader.next_record().unwrap().unwrap();
    let batch = ExportLogsServiceRequest::decode(rec.payload.as_slice()).unwrap();
    let lr = &batch.resource_logs[0].scope_logs[0].log_records[0];

    // time_unix_nano = first_seen (min), observed = last_seen (max).
    assert_eq!(lr.time_unix_nano, 100);
    assert_eq!(lr.observed_time_unix_nano, 900);

    let first_ns = lr.attributes.iter().find(|a| a.key == "dedup.first_seen_ns").unwrap();
    let last_ns = lr.attributes.iter().find(|a| a.key == "dedup.last_seen_ns").unwrap();
    if let Some(AnyValue { value: Some(Value::IntValue(v)) }) = &first_ns.value {
        assert_eq!(*v, 100);
    }
    if let Some(AnyValue { value: Some(Value::IntValue(v)) }) = &last_ns.value {
        assert_eq!(*v, 900);
    }
}

#[test]
fn passthrough_non_log_records() {
    // Write a log batch + a metrics record.
    let batch = make_batch("svc", vec![make_log_record("placeholder", 9, 100)]);
    let mut buf = Vec::new();
    {
        let mut writer = LogjetWriter::new(Cursor::new(&mut buf));
        let log_payload = batch.encode_to_vec();
        writer.push(RecordType::Logs, 0, 1000, &log_payload).unwrap();
        writer.push(RecordType::Metrics, 1, 1001, b"fake-metrics-payload").unwrap();
        writer.into_inner().unwrap();
    }

    let output = run_dedup(&buf, DedupMode::Exact);

    // Should have 2 records out: 1 deduped log + 1 passthrough metrics.
    let cursor = Cursor::new(&output);
    let mut reader = LogjetReader::new(cursor);
    let mut count = 0;
    let mut saw_metrics = false;
    while let Some(rec) = reader.next_record().unwrap() {
        count += 1;
        if rec.record_type == RecordType::Metrics {
            assert_eq!(rec.payload, b"fake-metrics-payload");
            saw_metrics = true;
        }
    }
    assert_eq!(count, 2);
    assert!(saw_metrics);
}

#[test]
fn empty_input_produces_empty_output() {
    let input = write_logjet(&[]);
    let output = run_dedup(&input, DedupMode::Exact);
    let groups = read_groups(&output);
    assert!(groups.is_empty());
}

#[test]
fn dedup_stats_reduction_percentage() {
    let stats = crate::dedup::DedupStats { total_records: 100, group_count: 25 };
    let pct = stats.reduction_pct();
    assert!((pct - 75.0).abs() < 0.01);
}

#[test]
fn full_mode_runs_drain3_for_small_residual_sets() {
    let batch = make_batch(
        "svc-a",
        vec![make_log_record("error in section alpha at line 42", 9, 100), make_log_record("error in section beta at line 99", 9, 200)],
    );
    let input = write_logjet(&[batch]);
    let output = run_dedup(&input, DedupMode::Full);
    let groups = read_groups(&output);

    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].1, 2);
}
