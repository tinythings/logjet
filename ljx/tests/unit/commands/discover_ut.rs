use std::fs::File;
use std::io::BufWriter;

use logjet::{LogjetWriter, RecordType, WriterConfig};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope, KeyValue};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message;

use super::{DiscoverySummary, all_counts, paged_entries, scan_entry, top_counts};
use crate::dataset::Dataset;
use crate::dataset_index::sidecar_path;
use crate::predicate::PredicateArgs;

fn write_logjet(path: &std::path::Path, rows: &[(u64, u64, &str, &str, &str)]) {
    let file = File::create(path).expect("create logjet");
    let mut writer = LogjetWriter::with_config(BufWriter::new(file), WriterConfig { block_target_size: 128, ..WriterConfig::default() });
    for (seq, ts, service, severity, body) in rows {
        let batch = ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                resource: Some(Resource {
                    attributes: vec![KeyValue {
                        key: "service.name".to_string(),
                        value: Some(AnyValue { value: Some(Value::StringValue((*service).to_string())) }),
                        ..Default::default()
                    }],
                    dropped_attributes_count: 0,
                    entity_refs: Vec::new(),
                }),
                scope_logs: vec![ScopeLogs {
                    scope: Some(InstrumentationScope {
                        name: "discover-ut".to_string(),
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
fn discover_scan_counts_services_severities_and_span() {
    let path = std::env::temp_dir().join(format!("ljx-discover-ut-{}.logjet", std::process::id()));
    write_logjet(&path, &[(1, 100, "api", "INFO", "started"), (2, 200, "api", "ERROR", "failed"), (3, 300, "worker", "ERROR", "retried")]);

    let dataset = Dataset::from_inputs(std::slice::from_ref(&path)).unwrap();
    let predicate = PredicateArgs::default().build().unwrap();
    let file = scan_entry(&dataset.entries()[0], &predicate, None, None).unwrap();
    let mut summary = DiscoverySummary::default();
    summary.merge(&file);

    assert_eq!(file.row.records_scanned, 3);
    assert_eq!(file.row.records_matched, 3);
    assert_eq!(file.row.log_events, 3);
    assert_eq!(file.row.time_span_unix_ns.first, Some(100));
    assert_eq!(file.row.time_span_unix_ns.last, Some(300));

    let top = top_counts(&summary.services, 1);
    assert_eq!(top[0].value, "api");
    assert_eq!(top[0].count, 2);
    let severities = all_counts(&summary.severities);
    assert!(severities.iter().any(|row| row.value == "ERROR" && row.count == 2));
    assert!(severities.iter().any(|row| row.value == "INFO" && row.count == 1));

    let _ = std::fs::remove_file(sidecar_path(&path));
    let _ = std::fs::remove_file(path);
}

#[test]
fn discover_paginates_manifest_entries() {
    let dir = std::env::temp_dir().join(format!("ljx-discover-page-ut-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.logjet");
    let b = dir.join("b.logjet");
    let c = dir.join("c.logjet");
    write_logjet(&a, &[(1, 100, "a", "INFO", "a")]);
    write_logjet(&b, &[(2, 200, "b", "INFO", "b")]);
    write_logjet(&c, &[(3, 300, "c", "INFO", "c")]);

    let dataset = Dataset::from_inputs(std::slice::from_ref(&dir)).unwrap();
    let page = paged_entries(&dataset, 1, Some(1)).unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].path, b);

    for path in [&a, &b, &c] {
        let _ = std::fs::remove_file(sidecar_path(path));
        let _ = std::fs::remove_file(path);
    }
    let _ = std::fs::remove_dir(dir);
}
