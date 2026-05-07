use std::io::Write;
use std::path::PathBuf;

use logjet::{LogjetWriter, RecordType, WriterConfig};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope, KeyValue};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message;
use serde_json::Value as JsonValue;

use crate::commands::export::run_query_ndjson_multi_with_writer;
use crate::predicate::PredicateArgs;

fn write_logjet(path: &std::path::Path, rows: &[(u64, u64, &str, &str, &str)]) {
    let file = std::fs::File::create(path).expect("create logjet");
    let mut writer = LogjetWriter::with_config(std::io::BufWriter::new(file), WriterConfig { block_target_size: 128, ..WriterConfig::default() });
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
                        name: "top-level-ut".to_string(),
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
    out.flush().expect("flush");
}

#[test]
fn top_level_query_grep_scans_multiple_inputs() {
    let dir = std::env::temp_dir().join(format!("ljx-top-query-ut-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.logjet");
    let b = dir.join("b.logjet");
    write_logjet(&a, &[(1, 100, "api", "INFO", "hello"), (2, 200, "api", "ERROR", "bad")]);
    write_logjet(&b, &[(3, 300, "worker", "INFO", "ok"), (4, 400, "worker", "ERROR", "timeout")]);

    let predicate = PredicateArgs { grep: vec!["timeout|bad".to_string()], ..PredicateArgs::default() }.build().unwrap();
    let mut output = Vec::new();
    run_query_ndjson_multi_with_writer(&[PathBuf::from(&a), PathBuf::from(&b)], &predicate, &[], None, &mut output).unwrap();

    let text = String::from_utf8(output).unwrap();
    let rows = text.lines().map(|line| serde_json::from_str::<JsonValue>(line).unwrap()).collect::<Vec<_>>();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|row| row.get("body") == Some(&JsonValue::String("bad".to_string()))));
    assert!(rows.iter().any(|row| row.get("body") == Some(&JsonValue::String("timeout".to_string()))));

    let _ = std::fs::remove_file(&a);
    let _ = std::fs::remove_file(&b);
    let _ = std::fs::remove_dir(&dir);
}
