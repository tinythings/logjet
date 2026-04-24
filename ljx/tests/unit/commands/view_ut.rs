use super::{
    DedupUpdate, DetailRecord, EntryMeta, ExportField, Focus, MODAL_ATTR_ENTRY_LIMIT_PER_KIND, ViewApp, create_temp_path, extract_otlp_log_message,
    format_summary, open_temp_spool_pair, read_spool_record, render_modal_info_entries, render_modal_message, text_preview, write_spool_record,
};
use crate::cli::{ViewArgs, ViewOrderArg};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use logjet::OwnedRecord;
use logjet::{LogjetWriter, RecordType};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope, KeyValue};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message;
use serde_json::Value as JsonValue;
use std::fs::File;
use std::io::BufWriter;
use std::sync::mpsc;
use std::time::Duration;

#[test]
fn text_preview_flattens_newlines() {
    assert_eq!(text_preview(b"hello\nworld", 32), "hello world");
}

#[test]
fn summary_uses_trimmed_single_line_preview() {
    let detail = DetailRecord {
        meta: EntryMeta { offset: 0, record_type: RecordType::Logs, seq: 7, ts_unix_ns: 9, payload_len: 13, source_path: "a.logjet".into() },
        payload: b"line one\nline two".to_vec(),
    };
    let summary = format_summary(&detail, false);
    assert_eq!(summary, "line one line two");
}

#[test]
fn summary_prefers_decoded_otlp_log_message() {
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
        meta: EntryMeta { offset: 0, record_type: RecordType::Logs, seq: 1, ts_unix_ns: 2, payload_len: payload.len() as u64, source_path: "a.logjet".into() },
        payload,
    };

    assert_eq!(extract_otlp_log_message(&detail.payload).as_deref(), Some("hello from body"));
    assert_eq!(format_summary(&detail, false), "hello from body");
}

#[test]
fn modal_falls_back_to_raw_payload() {
    let detail = DetailRecord {
        meta: EntryMeta { offset: 0, record_type: RecordType::Metrics, seq: 1, ts_unix_ns: 2, payload_len: 5, source_path: "a.logjet".into() },
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
                entity_refs: Vec::new(),
            }),
            scope_logs: vec![ScopeLogs {
                scope: Some(InstrumentationScope {
                    name: "liblogjet".to_string(),
                    version: String::new(),
                    attributes: vec![KeyValue {
                        key: "demo.channel".to_string(),
                        value: Some(AnyValue {
                            value: Some(Value::ArrayValue(opentelemetry_proto::tonic::common::v1::ArrayValue {
                                values: vec![
                                    AnyValue { value: Some(Value::StringValue("de".to_string())) },
                                    AnyValue { value: Some(Value::StringValue("eso".to_string())) },
                                ],
                            })),
                        }),
                    }],
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
        meta: EntryMeta { offset: 0, record_type: RecordType::Logs, seq: 1, ts_unix_ns: 2, payload_len: payload.len() as u64, source_path: "a.logjet".into() },
        payload,
    };

    let entries = render_modal_info_entries(&detail);
    assert!(entries.iter().any(|(key, value)| key == "resource.service.name" && value == "cpp-appliance"));
    assert!(entries.iter().any(|(key, value)| key == "scope.demo.channel" && value == "de"));
    assert!(entries.iter().any(|(key, value)| key.is_empty() && value == "eso"));
    assert!(entries.iter().any(|(key, value)| key == "record.character" && value == "Bender"));
}

#[test]
fn modal_info_caps_attribute_entries_per_kind() {
    let record_attributes = (0..40)
        .map(|index| KeyValue { key: format!("custom.{index}"), value: Some(AnyValue { value: Some(Value::StringValue(format!("value-{index}"))) }) })
        .collect::<Vec<_>>();
    let scope_attributes = (0..40)
        .map(|index| KeyValue {
            key: format!("scope.custom.{index}"),
            value: Some(AnyValue { value: Some(Value::StringValue(format!("scope-value-{index}"))) }),
        })
        .collect::<Vec<_>>();
    let batch = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource { attributes: Vec::new(), dropped_attributes_count: 0, entity_refs: Vec::new() }),
            scope_logs: vec![ScopeLogs {
                scope: Some(InstrumentationScope {
                    name: "liblogjet".to_string(),
                    version: String::new(),
                    attributes: scope_attributes,
                    dropped_attributes_count: 0,
                }),
                log_records: vec![LogRecord {
                    time_unix_nano: 0,
                    observed_time_unix_nano: 0,
                    severity_number: 0,
                    severity_text: "INFO".to_string(),
                    body: Some(AnyValue { value: Some(Value::StringValue("hello from cpp".to_string())) }),
                    attributes: record_attributes,
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
        meta: EntryMeta { offset: 0, record_type: RecordType::Logs, seq: 1, ts_unix_ns: 2, payload_len: payload.len() as u64, source_path: "a.logjet".into() },
        payload,
    };

    let entries = render_modal_info_entries(&detail);
    let scope_entries = entries.iter().filter(|(key, _)| key.starts_with("scope.scope.custom.")).count();
    let record_entries = entries.iter().filter(|(key, _)| key.starts_with("record.custom.")).count();
    assert_eq!(scope_entries, MODAL_ATTR_ENTRY_LIMIT_PER_KIND);
    assert!(entries.iter().any(|(key, value)| key == "scope.attrs.more" && value == "8 not shown"));
    assert_eq!(record_entries, MODAL_ATTR_ENTRY_LIMIT_PER_KIND);
    assert!(entries.iter().any(|(key, value)| key == "record.attrs.more" && value == "8 not shown"));
}

#[test]
fn temp_spool_reader_and_writer_use_independent_offsets() {
    let (path, mut reader, mut writer) = open_temp_spool_pair().unwrap();

    let first = OwnedRecord { record_type: RecordType::Logs, seq: 1, ts_unix_ns: 11, payload: b"first payload".to_vec() };
    let second = OwnedRecord { record_type: RecordType::Logs, seq: 2, ts_unix_ns: 22, payload: b"second payload".to_vec() };

    let first_meta = write_spool_record(&mut writer, &first, std::path::Path::new("star-one.logjet")).unwrap();
    let second_meta = write_spool_record(&mut writer, &second, std::path::Path::new("star-two.logjet")).unwrap();

    let reread_first = read_spool_record(&mut reader, first_meta).unwrap();
    let reread_second = read_spool_record(&mut reader, second_meta).unwrap();

    assert_eq!(reread_first.meta.seq, 1);
    assert_eq!(reread_first.payload, first.payload);
    assert_eq!(reread_second.meta.seq, 2);
    assert_eq!(reread_second.payload, second.payload);

    let _ = std::fs::remove_file(path);
}

fn make_view_app(input: std::path::PathBuf) -> ViewApp {
    if !input.exists() {
        std::fs::write(&input, b"").expect("create placeholder input");
    }
    ViewApp::new(ViewArgs { inputs: vec![input], dataset_order: ViewOrderArg::Concat, hex_payload: false, tail: false }).expect("view app")
}

fn make_view_app_inputs(inputs: Vec<std::path::PathBuf>) -> ViewApp {
    ViewApp::new(ViewArgs { inputs, dataset_order: ViewOrderArg::Concat, hex_payload: false, tail: false }).expect("view app")
}

fn make_view_app_inputs_order(inputs: Vec<std::path::PathBuf>, dataset_order: ViewOrderArg) -> ViewApp {
    ViewApp::new(ViewArgs { inputs, dataset_order, hex_payload: false, tail: false }).expect("view app")
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn write_test_logjet(path: &std::path::Path, bodies: &[&str]) {
    let batch = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: vec![KeyValue {
                    key: "service.name".to_string(),
                    value: Some(AnyValue { value: Some(Value::StringValue("fake-service".to_string())) }),
                }],
                dropped_attributes_count: 0,
                entity_refs: Vec::new(),
            }),
            scope_logs: vec![ScopeLogs {
                scope: Some(InstrumentationScope {
                    name: "fake-scope".to_string(),
                    version: String::new(),
                    attributes: Vec::new(),
                    dropped_attributes_count: 0,
                }),
                log_records: bodies
                    .iter()
                    .enumerate()
                    .map(|(i, body)| LogRecord {
                        time_unix_nano: (i as u64 + 1) * 100,
                        observed_time_unix_nano: (i as u64 + 1) * 100,
                        severity_number: 9,
                        severity_text: "INFO".to_string(),
                        body: Some(AnyValue { value: Some(Value::StringValue((*body).to_string())) }),
                        attributes: Vec::new(),
                        dropped_attributes_count: 0,
                        flags: 0,
                        trace_id: Vec::new(),
                        span_id: Vec::new(),
                        event_name: String::new(),
                    })
                    .collect(),
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };

    let file = File::create(path).expect("create input logjet");
    let mut writer = LogjetWriter::new(BufWriter::new(file));
    let payload = batch.encode_to_vec();
    writer.push(RecordType::Logs, 1, 100, &payload).expect("write record");
    let mut inner = writer.into_inner().expect("into inner");
    use std::io::Write;
    inner.flush().expect("flush");
}

fn write_test_logjet_rows(path: &std::path::Path, rows: &[(&str, &[&str], &str)]) {
    let file = File::create(path).expect("create input logjet");
    let mut writer = LogjetWriter::new(BufWriter::new(file));

    for (i, (body, channel, event_name)) in rows.iter().enumerate() {
        let batch = ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                resource: Some(Resource {
                    attributes: vec![KeyValue {
                        key: "service.name".to_string(),
                        value: Some(AnyValue { value: Some(Value::StringValue("fake-service".to_string())) }),
                    }],
                    dropped_attributes_count: 0,
                    entity_refs: Vec::new(),
                }),
                scope_logs: vec![ScopeLogs {
                    scope: Some(InstrumentationScope {
                        name: "fake-scope".to_string(),
                        version: String::new(),
                        attributes: vec![KeyValue {
                            key: "demo.channel".to_string(),
                            value: Some(AnyValue {
                                value: Some(Value::ArrayValue(opentelemetry_proto::tonic::common::v1::ArrayValue {
                                    values: channel.iter().map(|part| AnyValue { value: Some(Value::StringValue((*part).to_string())) }).collect(),
                                })),
                            }),
                        }],
                        dropped_attributes_count: 0,
                    }),
                    log_records: vec![LogRecord {
                        time_unix_nano: (i as u64 + 1) * 100,
                        observed_time_unix_nano: (i as u64 + 1) * 100,
                        severity_number: 9,
                        severity_text: "INFO".to_string(),
                        body: Some(AnyValue { value: Some(Value::StringValue((*body).to_string())) }),
                        attributes: Vec::new(),
                        dropped_attributes_count: 0,
                        flags: 0,
                        trace_id: Vec::new(),
                        span_id: Vec::new(),
                        event_name: (*event_name).to_string(),
                    }],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        };
        let payload = batch.encode_to_vec();
        writer.push(RecordType::Logs, i as u64 + 1, (i as u64 + 1) * 100, &payload).expect("write row");
    }

    let mut inner = writer.into_inner().expect("into inner");
    use std::io::Write;
    inner.flush().expect("flush");
}

fn write_test_logjet_records(path: &std::path::Path, rows: &[(u64, u64, &str)]) {
    let file = File::create(path).expect("create input logjet");
    let mut writer = LogjetWriter::new(BufWriter::new(file));

    for (seq, ts, body) in rows {
        let batch = ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                resource: Some(Resource {
                    attributes: vec![KeyValue {
                        key: "service.name".to_string(),
                        value: Some(AnyValue { value: Some(Value::StringValue("fake-service".to_string())) }),
                    }],
                    dropped_attributes_count: 0,
                    entity_refs: Vec::new(),
                }),
                scope_logs: vec![ScopeLogs {
                    scope: Some(InstrumentationScope {
                        name: "fake-scope".to_string(),
                        version: String::new(),
                        attributes: Vec::new(),
                        dropped_attributes_count: 0,
                    }),
                    log_records: vec![LogRecord {
                        time_unix_nano: *ts,
                        observed_time_unix_nano: *ts,
                        severity_number: 9,
                        severity_text: "INFO".to_string(),
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
        writer.push(RecordType::Logs, *seq, *ts, &batch.encode_to_vec()).expect("write row");
    }

    let mut inner = writer.into_inner().expect("into inner");
    use std::io::Write;
    inner.flush().expect("flush");
}

fn read_first_log_record(path: &std::path::Path) -> LogRecord {
    let bytes = std::fs::read(path).expect("read output file");
    let cursor = std::io::Cursor::new(bytes);
    let mut reader = logjet::LogjetReader::new(cursor);
    while let Some(rec) = reader.next_record().expect("next record") {
        if rec.record_type != RecordType::Logs {
            continue;
        }
        let batch = ExportLogsServiceRequest::decode(rec.payload.as_slice()).expect("decode batch");
        return batch.resource_logs[0].scope_logs[0].log_records[0].clone();
    }
    panic!("no log record in output");
}

fn find_attr_str(record: &LogRecord, key: &str) -> Option<String> {
    record.attributes.iter().find(|a| a.key == key).and_then(|a| match &a.value {
        Some(AnyValue { value: Some(Value::StringValue(s)) }) => Some(s.clone()),
        _ => None,
    })
}

fn wait_for_scan(app: &mut ViewApp) {
    for _ in 0..100 {
        app.drain_scan_updates().expect("drain scan");
        if app.current_scan.as_ref().map(|scan| scan.finished).unwrap_or(false) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("scan did not finish in time");
}

fn read_ndjson(path: &std::path::Path) -> Vec<JsonValue> {
    std::fs::read_to_string(path)
        .expect("read ndjson")
        .lines()
        .map(|line| serde_json::from_str::<JsonValue>(line).expect("parse ndjson line"))
        .collect()
}

#[test]
fn dedup_prompt_switches_behaviour_and_matcher_independently() {
    let input = create_temp_path().unwrap();
    let mut app = make_view_app(input.clone());
    app.open_dedup_prompt();

    assert!(matches!(app.focus, Focus::DedupPrompt));
    assert_eq!(app.dedup_behavior.label(), "distinct");
    assert_eq!(app.dedup_match_mode.label(), "canon");

    app.handle_dedup_prompt_key(key(KeyCode::Right)).unwrap();
    assert_eq!(app.dedup_behavior.label(), "collapse");
    assert_eq!(app.dedup_match_mode.label(), "canon");

    app.handle_dedup_prompt_key(key(KeyCode::Down)).unwrap();
    assert_eq!(app.dedup_behavior.label(), "collapse");
    assert_eq!(app.dedup_match_mode.label(), "full");

    let _ = std::fs::remove_file(input);
}

#[test]
fn dedup_worker_uses_selected_behaviour_and_matcher() {
    let input = create_temp_path().unwrap();
    write_test_logjet(&input, &["lapsed time is 2", "lapsed time is 3"]);

    let mut app = make_view_app(input.clone());
    app.start_dedup("dedup-out.logjet", crate::dedup::DedupMode::Collapse, crate::dedup::DedupMatchMode::Full);

    let rx = app.dedup_rx.take().expect("dedup receiver");
    let output_path = app.dedup_output_path.clone().expect("output path");

    loop {
        match rx.recv_timeout(Duration::from_secs(5)).expect("dedup update") {
            DedupUpdate::Done { .. } => break,
            DedupUpdate::Failed(err) => panic!("dedup failed: {err}"),
            DedupUpdate::Progress { .. } => {}
        }
    }

    let record = read_first_log_record(&output_path);
    assert_eq!(find_attr_str(&record, "dedup.behaviour").as_deref(), Some("collapse"));
    assert_eq!(find_attr_str(&record, "dedup.matcher").as_deref(), Some("drain3"));
    assert_eq!(find_attr_str(&record, "dedup.window").as_deref(), Some("consecutive-run"));

    let _ = std::fs::remove_file(input);
    let _ = std::fs::remove_file(output_path);
}

#[test]
fn dedup_progress_updates_target_and_eases_displayed_progress() {
    let input = create_temp_path().unwrap();
    let mut app = make_view_app(input.clone());
    let (tx, rx) = mpsc::channel();
    app.dedup_rx = Some(rx);
    app.dedup_progress = 0.10;
    app.dedup_progress_target = 0.10;

    tx.send(DedupUpdate::Progress { ratio: 0.80, phase: "running collapse / full".to_string() }).unwrap();
    drop(tx);

    app.drain_dedup_updates();

    assert_eq!(app.dedup_progress_target, 0.80);
    assert_eq!(app.dedup_phase, "running collapse / full");
    assert!(app.dedup_progress > 0.10);
    assert!(app.dedup_progress < 0.80);

    let _ = std::fs::remove_file(input);
}

#[test]
fn dedup_done_keeps_popup_open_until_enter() {
    let input = create_temp_path().unwrap();
    let output = create_temp_path().unwrap();
    write_test_logjet(&input, &["placeholder route update"]);
    write_test_logjet(&output, &["placeholder route update"]);

    let mut app = make_view_app(input.clone());
    let (tx, rx) = mpsc::channel();
    app.dedup_rx = Some(rx);
    app.dedup_output_path = Some(output.clone());
    app.focus = Focus::DedupProgress;

    tx.send(DedupUpdate::Done { total: 94102, groups: 9039, pct: 90.4 }).unwrap();
    drop(tx);

    app.drain_dedup_updates();

    assert!(matches!(app.focus, Focus::DedupProgress));
    assert_eq!(app.dedup_progress, 1.0);
    assert_eq!(app.dedup_phase, "OK");
    assert_eq!(app.dedup_completion_message.as_deref(), Some("94102 records → 9039 groups (90.4% reduction)"));

    let _ = std::fs::remove_file(input);
    let _ = std::fs::remove_file(output);
}

#[test]
fn dedup_progress_enter_opens_output_and_sets_status() {
    let input = create_temp_path().unwrap();
    let output = create_temp_path().unwrap();
    write_test_logjet(&input, &["placeholder route update"]);
    write_test_logjet(&output, &["placeholder route update"]);

    let mut app = make_view_app(input.clone());
    app.focus = Focus::DedupProgress;
    app.dedup_output_path = Some(output.clone());
    app.dedup_completion_message = Some("10 records → 2 groups (80.0% reduction)".to_string());
    app.dedup_progress = 1.0;
    app.dedup_progress_target = 1.0;
    app.dedup_phase = "OK".to_string();

    app.handle_dedup_progress_key(key(KeyCode::Enter)).unwrap();

    assert!(matches!(app.focus, Focus::List));
    assert!(app.status.contains("10 records → 2 groups (80.0% reduction)"));
    assert_eq!(app.input, output);
    assert!(app.dedup_completion_message.is_none());

    let _ = std::fs::remove_file(input);
    let _ = std::fs::remove_file(output);
}

#[test]
fn export_prompt_defaults_to_ndjson_and_all() {
    let input = create_temp_path().unwrap();
    write_test_logjet_rows(&input, &[("alpha", &["AndroidGeoLocationListener"], "demo.log.utf8")]);

    let mut app = make_view_app(input.clone());
    app.apply_filter().unwrap();
    wait_for_scan(&mut app);
    app.open_export_prompt().unwrap();

    assert!(matches!(app.focus, Focus::ExportPrompt));
    assert!(app.export_filename.ends_with(".ndjson"));
    assert_eq!(app.export_range, "all");

    let _ = std::fs::remove_file(input);
}

#[test]
fn multi_input_view_scans_star_wars_files_in_stable_order() {
    let base = create_temp_path().unwrap();
    let dir = base.parent().unwrap().to_path_buf();
    let id = base.file_name().unwrap().to_string_lossy().into_owned();
    let alderaan = dir.join(format!("alderaan-{id}.logjet"));
    let death_star = dir.join(format!("death-star-{id}.logjet"));
    write_test_logjet(&death_star, &["Vader patrol report"]);
    write_test_logjet(&alderaan, &["Leia rebel briefing"]);

    let mut app = make_view_app_inputs(vec![death_star.clone(), alderaan.clone()]);
    app.apply_filter().unwrap();
    wait_for_scan(&mut app);

    assert_eq!(app.entries.len(), 2);
    let first = app.summary_for(0).unwrap();
    let second = app.summary_for(1).unwrap();
    assert_eq!(first.message, "Leia rebel briefing");
    assert_eq!(second.message, "Vader patrol report");

    let _ = std::fs::remove_file(base);
    let _ = std::fs::remove_file(alderaan);
    let _ = std::fs::remove_file(death_star);
}

#[test]
fn multi_input_view_merge_seq_orders_by_sequence() {
    let base = create_temp_path().unwrap();
    let dir = base.parent().unwrap().to_path_buf();
    let id = base.file_name().unwrap().to_string_lossy().into_owned();
    let hoth = dir.join(format!("hoth-{id}.logjet"));
    let endor = dir.join(format!("endor-{id}.logjet"));
    write_test_logjet_records(&hoth, &[(9, 1000, "Luke on Hoth")]);
    write_test_logjet_records(&endor, &[(3, 9000, "Leia on Endor")]);

    let mut app = make_view_app_inputs_order(vec![hoth.clone(), endor.clone()], ViewOrderArg::MergeSeq);
    app.apply_filter().unwrap();
    wait_for_scan(&mut app);

    assert_eq!(app.summary_for(0).unwrap().message, "Leia on Endor");
    assert_eq!(app.summary_for(1).unwrap().message, "Luke on Hoth");

    let _ = std::fs::remove_file(base);
    let _ = std::fs::remove_file(hoth);
    let _ = std::fs::remove_file(endor);
}

#[test]
fn multi_input_view_merge_ts_orders_by_timestamp() {
    let base = create_temp_path().unwrap();
    let dir = base.parent().unwrap().to_path_buf();
    let id = base.file_name().unwrap().to_string_lossy().into_owned();
    let mustafar = dir.join(format!("mustafar-{id}.logjet"));
    let naboo = dir.join(format!("naboo-{id}.logjet"));
    write_test_logjet_records(&mustafar, &[(2, 9000, "Vader on Mustafar")]);
    write_test_logjet_records(&naboo, &[(7, 1200, "Padme on Naboo")]);

    let mut app = make_view_app_inputs_order(vec![mustafar.clone(), naboo.clone()], ViewOrderArg::MergeTs);
    app.apply_filter().unwrap();
    wait_for_scan(&mut app);

    assert_eq!(app.summary_for(0).unwrap().message, "Padme on Naboo");
    assert_eq!(app.summary_for(1).unwrap().message, "Vader on Mustafar");

    let _ = std::fs::remove_file(base);
    let _ = std::fs::remove_file(mustafar);
    let _ = std::fs::remove_file(naboo);
}

#[test]
fn current_record_filename_tracks_physical_source_file() {
    let base = create_temp_path().unwrap();
    let dir = base.parent().unwrap().to_path_buf();
    let id = base.file_name().unwrap().to_string_lossy().into_owned();
    let jedha = dir.join(format!("jedha-{id}.logjet"));
    let scarif = dir.join(format!("scarif-{id}.logjet"));
    write_test_logjet_records(&jedha, &[(1, 100, "Jyn on Jedha")]);
    write_test_logjet_records(&scarif, &[(2, 200, "Cassian on Scarif")]);

    let mut app = make_view_app_inputs(vec![jedha.clone(), scarif.clone()]);
    app.apply_filter().unwrap();
    wait_for_scan(&mut app);

    assert_eq!(app.current_record_filename().as_deref(), Some(jedha.file_name().and_then(|s| s.to_str()).unwrap()));
    app.selected = 1;
    assert_eq!(app.current_record_filename().as_deref(), Some(scarif.file_name().and_then(|s| s.to_str()).unwrap()));

    let _ = std::fs::remove_file(base);
    let _ = std::fs::remove_file(jedha);
    let _ = std::fs::remove_file(scarif);
}

#[test]
fn export_format_cycle_updates_filename_extension() {
    let input = create_temp_path().unwrap();
    write_test_logjet_rows(&input, &[("alpha", &["AndroidGeoLocationListener"], "demo.log.utf8")]);

    let mut app = make_view_app(input.clone());
    app.apply_filter().unwrap();
    wait_for_scan(&mut app);
    app.export_formats = vec![super::ExportFormatChoice::ndjson(), super::ExportFormatChoice::from_plugin_name("parquet".to_string())];
    app.export_format_index = 0;
    app.export_filename = "export-out.ndjson".to_string();
    app.export_filename_cursor = app.export_filename.len();

    app.cycle_export_format(1);

    assert_eq!(app.current_export_format().label(), "parquet");
    assert_eq!(app.export_filename, "export-out.parquet");

    let _ = std::fs::remove_file(input);
}

#[test]
fn export_prompt_supports_cursor_navigation_and_mid_string_editing() {
    let input = create_temp_path().unwrap();
    write_test_logjet_rows(&input, &[("alpha", &["AndroidGeoLocationListener"], "demo.log.utf8")]);

    let mut app = make_view_app(input.clone());
    app.apply_filter().unwrap();
    wait_for_scan(&mut app);
    app.open_export_prompt().unwrap();

    app.export_field = ExportField::Range;
    app.export_range = "all".to_string();
    app.export_range_cursor = 1;
    app.handle_export_prompt_key(KeyEvent::from(KeyCode::Delete)).unwrap();
    assert_eq!(app.export_range, "al");
    assert_eq!(app.export_range_cursor, 1);

    app.handle_export_prompt_key(KeyEvent::from(KeyCode::Left)).unwrap();
    app.handle_export_prompt_key(KeyEvent::from(KeyCode::Char('c'))).unwrap();
    assert_eq!(app.export_range, "cal");
    assert_eq!(app.export_range_cursor, 1);

    app.handle_export_prompt_key(KeyEvent::from(KeyCode::End)).unwrap();
    app.handle_export_prompt_key(KeyEvent::from(KeyCode::Char('l'))).unwrap();
    assert_eq!(app.export_range, "call");

    app.handle_export_prompt_key(KeyEvent::from(KeyCode::Home)).unwrap();
    assert_eq!(app.export_range_cursor, 0);

    let _ = std::fs::remove_file(input);
}

#[test]
fn export_selection_accepts_all_amount_and_range() {
    assert_eq!(super::parse_export_selection("all", 50, 0).unwrap(), (0, 50));
    assert_eq!(super::parse_export_selection("aLl", 50, 0).unwrap(), (0, 50));
    assert_eq!(super::parse_export_selection("a", 50, 0).unwrap(), (0, 50));
    assert_eq!(super::parse_export_selection("current", 50, 7).unwrap(), (7, 8));
    assert_eq!(super::parse_export_selection("CuRrEnT", 50, 7).unwrap(), (7, 8));
    assert_eq!(super::parse_export_selection("c", 50, 7).unwrap(), (7, 8));
    assert_eq!(super::parse_export_selection("0", 50, 7).unwrap(), (7, 8));
    assert_eq!(super::parse_export_selection("3", 50, 0).unwrap(), (0, 3));
    assert_eq!(super::parse_export_selection("2-4", 50, 0).unwrap(), (1, 4));
}

#[test]
fn export_current_results_writes_ndjson_from_filtered_view() {
    let input = create_temp_path().unwrap();
    let output = input.parent().unwrap().join("export-out.ndjson");
    write_test_logjet_rows(
        &input,
        &[
            ("first gps", &["AndroidGeoLocationListener"], "demo.log.utf8"),
            ("second gps", &["AndroidGeoLocationListener"], "demo.log.utf8"),
            ("third gps", &["AndroidGeoLocationListener"], "demo.log.utf8"),
        ],
    );

    let mut app = make_view_app(input.clone());
    app.apply_filter().unwrap();
    wait_for_scan(&mut app);
    app.export_filename = "export-out.ndjson".to_string();
    app.export_range = "2-3".to_string();
    app.export_current_results().unwrap();

    let docs = read_ndjson(&output);
    assert_eq!(docs.len(), 2);
    assert_eq!(docs[0]["body"], "second gps");
    assert_eq!(docs[0]["event_name"], "demo.log.utf8");
    assert_eq!(docs[0]["severity_text"], "INFO");
    assert_eq!(docs[0]["severity_number"], 9);
    assert_eq!(docs[0]["demo_channel"][0], "AndroidGeoLocationListener");
    assert_eq!(docs[1]["body"], "third gps");

    let _ = std::fs::remove_file(input);
    let _ = std::fs::remove_file(output);
}

#[test]
fn export_current_results_can_export_selected_row_only() {
    let input = create_temp_path().unwrap();
    let output = input.parent().unwrap().join("export-current.ndjson");
    write_test_logjet_rows(
        &input,
        &[
            ("first gps", &["AndroidGeoLocationListener"], "demo.log.utf8"),
            ("second gps", &["AndroidGeoLocationListener"], "demo.log.utf8"),
            ("third gps", &["AndroidGeoLocationListener"], "demo.log.utf8"),
        ],
    );

    let mut app = make_view_app(input.clone());
    app.apply_filter().unwrap();
    wait_for_scan(&mut app);
    app.selected = 1;
    app.export_filename = "export-current.ndjson".to_string();
    app.export_range = "current".to_string();
    app.export_current_results().unwrap();

    let docs = read_ndjson(&output);
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0]["body"], "second gps");
    assert_eq!(docs[0]["severity_text"], "INFO");

    let _ = std::fs::remove_file(input);
    let _ = std::fs::remove_file(output);
}
