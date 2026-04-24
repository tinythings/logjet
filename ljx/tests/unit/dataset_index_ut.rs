use std::fs::File;
use std::io::BufWriter;

use logjet::{LogjetWriter, RecordType, WriterConfig};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope, KeyValue};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message;

use crate::dataset::Dataset;
use crate::dataset_index::sidecar_path;
use crate::predicate::PredicateArgs;

fn write_star_wars_logjet(path: &std::path::Path, rows: &[(u64, u64, &str, &str, &str)], block_target_size: usize) {
    let file = File::create(path).expect("create logjet");
    let mut writer = LogjetWriter::with_config(BufWriter::new(file), WriterConfig { block_target_size, ..WriterConfig::default() });
    for (seq, ts, service, severity, body) in rows {
        let batch = ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                resource: Some(Resource {
                    attributes: vec![KeyValue {
                        key: "service.name".to_string(),
                        value: Some(AnyValue { value: Some(Value::StringValue((*service).to_string())) }),
                    }],
                    dropped_attributes_count: 0,
                    entity_refs: Vec::new(),
                }),
                scope_logs: vec![ScopeLogs {
                    scope: Some(InstrumentationScope {
                        name: "holonet".to_string(),
                        version: String::new(),
                        attributes: Vec::new(),
                        dropped_attributes_count: 0,
                    }),
                    log_records: vec![LogRecord {
                        time_unix_nano: *ts,
                        observed_time_unix_nano: *ts,
                        severity_number: 9,
                        severity_text: (*severity).to_string(),
                        body: Some(AnyValue { value: Some(Value::StringValue((*body).to_string())) }),
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
        writer.push(RecordType::Logs, *seq, *ts, &batch.encode_to_vec()).expect("push row");
    }
    let mut out = writer.into_inner().expect("into inner");
    use std::io::Write;
    out.flush().expect("flush");
}

#[test]
fn sidecar_index_builds_bounds_and_log_summaries() {
    let path = std::env::temp_dir().join(format!("endor-sidecar-{}.logjet", std::process::id()));
    write_star_wars_logjet(
        &path,
        &[
            (4, 1_000, "rebel-fleet", "WARN", "Endor shield pulse"),
            (9, 5_000, "rebel-fleet", "ERROR", "Death Star reactor breach"),
        ],
        32,
    );

    let ds = Dataset::from_inputs(std::slice::from_ref(&path)).unwrap();
    let entry = &ds.entries()[0];
    let idx = entry.index.as_ref().expect("sidecar index");
    assert!(sidecar_path(&path).exists());
    assert_eq!(entry.first_seq, Some(4));
    assert_eq!(entry.last_seq, Some(9));
    assert_eq!(entry.first_ts_unix_ns, Some(1_000));
    assert_eq!(entry.last_ts_unix_ns, Some(5_000));
    assert!(idx.summary.services.iter().any(|v| v == "rebel-fleet"));
    assert!(idx.summary.severities.iter().any(|v| v == "WARN"));
    assert!(idx.blocks.len() >= 2);

    let _ = std::fs::remove_file(sidecar_path(&path));
    let _ = std::fs::remove_file(path);
}

#[test]
fn sidecar_index_rebuilds_when_source_changes() {
    let path = std::env::temp_dir().join(format!("mustafar-sidecar-{}.logjet", std::process::id()));
    write_star_wars_logjet(&path, &[(2, 200, "empire", "INFO", "Vader arrives")], 128);
    let first = Dataset::from_inputs(std::slice::from_ref(&path)).unwrap();
    assert_eq!(first.entries()[0].last_ts_unix_ns, Some(200));

    write_star_wars_logjet(
        &path,
        &[
            (3, 300, "empire", "INFO", "Vader arrives with probe droids"),
            (8, 800, "empire", "ERROR", "Mustafar lava surge alert"),
        ],
        128,
    );
    let second = Dataset::from_inputs(std::slice::from_ref(&path)).unwrap();
    assert_eq!(second.entries()[0].last_seq, Some(8));
    assert_eq!(second.entries()[0].last_ts_unix_ns, Some(800));

    let _ = std::fs::remove_file(sidecar_path(&path));
    let _ = std::fs::remove_file(path);
}

#[test]
fn sidecar_summary_prunes_timestamp_and_type_misses() {
    let path = std::env::temp_dir().join(format!("hoth-sidecar-{}.logjet", std::process::id()));
    write_star_wars_logjet(&path, &[(1, 100, "rebel-base", "INFO", "Echo Base ready")], 128);

    let ds = Dataset::from_inputs(std::slice::from_ref(&path)).unwrap();
    let idx = ds.entries()[0].index.as_ref().expect("sidecar index");
    let ts_pred = PredicateArgs { ts_min: Some(500), ..PredicateArgs::default() }.build().unwrap();
    let type_pred = PredicateArgs { record_type: Some(crate::predicate::RecordKind::Metrics), ..PredicateArgs::default() }.build().unwrap();

    assert!(!idx.summary.may_match(&ts_pred));
    assert!(!idx.summary.may_match(&type_pred));

    let _ = std::fs::remove_file(sidecar_path(&path));
    let _ = std::fs::remove_file(path);
}
