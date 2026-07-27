use super::{FilterMode, PredicateArgs, RecordKind, parse_filter_query};
use logjet::{OwnedRecord, RecordType};

fn sample_record(payload: &[u8]) -> OwnedRecord {
    OwnedRecord { record_type: RecordType::Logs, seq: 42, ts_unix_ns: 1_700_000_000, payload: payload.to_vec() }
}

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope, KeyValue};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message;

fn make_cel_test_payload(body: &str, severity_number: i32, severity_text: &str, service_name: &str) -> Vec<u8> {
    let batch = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: vec![KeyValue {
                    key: "service.name".to_string(),
                    value: Some(AnyValue { value: Some(Value::StringValue(service_name.to_string())) }),
                }],
                dropped_attributes_count: 0,
                entity_refs: Vec::new(),
            }),
            scope_logs: vec![ScopeLogs {
                scope: Some(InstrumentationScope {
                    name: "test".to_string(),
                    version: "1.0.0".to_string(),
                    attributes: vec![],
                    dropped_attributes_count: 0,
                }),
                log_records: vec![LogRecord {
                    body: Some(AnyValue { value: Some(Value::StringValue(body.to_string())) }),
                    severity_number,
                    severity_text: severity_text.to_string(),
                    ..Default::default()
                }],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };
    batch.encode_to_vec()
}

fn cel_sample(body: &str, severity_number: i32, severity_text: &str) -> OwnedRecord {
    let payload = make_cel_test_payload(body, severity_number, severity_text, "test-svc");
    sample_record(&payload)
}

#[test]
fn fixed_string_match_is_literal() {
    let predicate = PredicateArgs { fixed_string: vec!["java.crap.failed".to_string()], ..PredicateArgs::default() }.build().unwrap();

    assert!(predicate.matches(&sample_record(b"xxx java.crap.failed yyy")));
    assert!(!predicate.matches(&sample_record(b"javaXcrapXfailed")));
}

#[test]
fn regex_match_supports_wildcards() {
    let predicate = PredicateArgs { grep: vec![r"java\..*\.bs".to_string()], ..PredicateArgs::default() }.build().unwrap();

    assert!(predicate.matches(&sample_record(b"java.very.long.bs")));
    assert!(!predicate.matches(&sample_record(b"java.very.long.cs")));
}

#[test]
fn ignore_case_applies_to_fixed_string_and_regex() {
    let fixed = PredicateArgs { fixed_string: vec!["error".to_string()], ignore_case: true, ..PredicateArgs::default() }.build().unwrap();
    let regex = PredicateArgs { grep: vec!["error".to_string()], ignore_case: true, ..PredicateArgs::default() }.build().unwrap();

    let record = sample_record(b"prefix eRrOr suffix");
    assert!(fixed.matches(&record));
    assert!(regex.matches(&record));
}

#[test]
fn matcher_combines_with_record_fields() {
    let predicate = PredicateArgs {
        record_type: Some(RecordKind::Logs),
        seq_min: Some(40),
        seq_max: Some(45),
        ts_min: Some(1_699_999_999),
        ts_max: Some(1_700_000_001),
        fixed_string: vec!["hello".to_string()],
        ..PredicateArgs::default()
    }
    .build()
    .unwrap();

    assert!(predicate.matches(&sample_record(b"hello world")));
    assert!(!predicate.matches(&sample_record(b"bye world")));
}

#[test]
fn invalid_regex_is_reported() {
    let error = PredicateArgs { grep: vec!["(".to_string()], ..PredicateArgs::default() }.build().unwrap_err();

    assert!(error.to_string().contains("invalid payload matcher"));
}

#[test]
fn repeated_fixed_strings_are_combined_with_and_semantics() {
    let predicate = PredicateArgs { fixed_string: vec!["foo".to_string(), "bar".to_string()], ..PredicateArgs::default() }.build().unwrap();

    assert!(predicate.matches(&sample_record(b"prefix foo and bar suffix")));
    assert!(!predicate.matches(&sample_record(b"only foo here")));
}

#[test]
fn repeated_regexes_are_combined_with_and_semantics() {
    let predicate = PredicateArgs { grep: vec!["foo".to_string(), "bar|baz".to_string()], ..PredicateArgs::default() }.build().unwrap();

    assert!(predicate.matches(&sample_record(b"foo plus baz")));
    assert!(predicate.matches(&sample_record(b"bar comes with foo")));
    assert!(!predicate.matches(&sample_record(b"foo alone")));
}

#[test]
fn fixed_string_and_regex_can_be_mixed() {
    let predicate =
        PredicateArgs { fixed_string: vec!["customer-123".to_string()], grep: vec!["error|panic".to_string()], ..PredicateArgs::default() }
            .build()
            .unwrap();

    assert!(predicate.matches(&sample_record(b"panic for customer-123")));
    assert!(!predicate.matches(&sample_record(b"panic for customer-999")));
}

#[test]
fn parse_filter_query_treats_bare_text_as_fixed_string() {
    let predicate = parse_filter_query("hello world", FilterMode::Strings).unwrap();
    assert!(predicate.matches(&sample_record(b"say hello world now")));
    assert!(!predicate.matches(&sample_record(b"say hello now")));
}

#[test]
fn parse_filter_query_supports_cli_style_flags() {
    let predicate = parse_filter_query(r#"--type logs -e "error|panic" -i"#, FilterMode::Strings).unwrap();
    assert!(predicate.matches(&sample_record(b"PANIC happened")));
    assert!(!predicate.matches(&OwnedRecord { record_type: RecordType::Metrics, seq: 42, ts_unix_ns: 1_700_000_000, payload: b"panic".to_vec() }));
}

#[test]
fn parse_filter_query_uses_regex_mode_for_bare_text() {
    let predicate = parse_filter_query("reb.*", FilterMode::Regex).unwrap();
    assert!(predicate.matches(&sample_record(b"rebooted node")));
    assert!(!predicate.matches(&sample_record(b"stopped node")));
}

#[test]
fn cel_severity_number_match() {
    let predicate = PredicateArgs { cel: vec!["severity_number == 17".to_string()], ..PredicateArgs::default() }.build().unwrap();
    assert!(predicate.matches(&cel_sample("fatal error", 17, "FATAL")));
    assert!(!predicate.matches(&cel_sample("info msg", 9, "INFO")));
}

#[test]
fn cel_body_contains_match() {
    let predicate =
        PredicateArgs { cel: vec!["body.contains(\"timeout\")".to_string()], ..PredicateArgs::default() }.build().unwrap();
    assert!(predicate.matches(&cel_sample("connection timeout", 17, "ERROR")));
    assert!(!predicate.matches(&cel_sample("connection ok", 9, "INFO")));
}

#[test]
fn cel_service_name_match() {
    let predicate =
        PredicateArgs { cel: vec!["service_name == \"test-svc\"".to_string()], ..PredicateArgs::default() }.build().unwrap();
    assert!(predicate.matches(&cel_sample("msg", 9, "INFO")));
}

#[test]
fn cel_combined_with_fixed_string() {
    let predicate = PredicateArgs {
        cel: vec!["severity_number >= 13".to_string()],
        fixed_string: vec!["timeout".to_string()],
        ..PredicateArgs::default()
    }
    .build()
    .unwrap();
    assert!(predicate.matches(&cel_sample("connection timeout after 30s", 17, "ERROR")));
    assert!(!predicate.matches(&cel_sample("connection timeout after 30s", 9, "INFO")));
}

#[test]
fn cel_invalid_expression_is_reported() {
    let error = PredicateArgs { cel: vec!["@$#% invalid".to_string()], ..PredicateArgs::default() }.build().unwrap_err();
    assert!(error.to_string().contains("invalid CEL expression"));
}

#[test]
fn cel_repeated_exprs_are_combined_with_and_semantics() {
    let predicate = PredicateArgs {
        cel: vec![
            "severity_number >= 13".to_string(),
            "body.contains(\"timeout\")".to_string(),
        ],
        ..PredicateArgs::default()
    }
    .build()
    .unwrap();
    assert!(predicate.matches(&cel_sample("connection timeout", 17, "ERROR")));
    assert!(!predicate.matches(&cel_sample("connection timeout", 9, "INFO")));
    assert!(!predicate.matches(&cel_sample("connection ok", 17, "ERROR")));
}

#[test]
fn parse_filter_query_uses_cel_mode_for_bare_text() {
    let predicate = parse_filter_query("severity_number >= 13", FilterMode::Cel).unwrap();
    let payload = make_cel_test_payload("error msg", 13, "ERROR", "test-svc");
    let record = sample_record(&payload);
    assert!(predicate.matches(&record));
}
