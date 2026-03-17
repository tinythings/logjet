use super::{FilterMode, PredicateArgs, RecordKind, parse_filter_query};
use logjet::{OwnedRecord, RecordType};

fn sample_record(payload: &[u8]) -> OwnedRecord {
    OwnedRecord { record_type: RecordType::Logs, seq: 42, ts_unix_ns: 1_700_000_000, payload: payload.to_vec() }
}

#[test]
fn fixed_string_match_is_literal() {
    let predicate = PredicateArgs { fixed_string: Some("java.crap.failed".to_string()), ..PredicateArgs::default() }.build().unwrap();

    assert!(predicate.matches(&sample_record(b"xxx java.crap.failed yyy")));
    assert!(!predicate.matches(&sample_record(b"javaXcrapXfailed")));
}

#[test]
fn regex_match_supports_wildcards() {
    let predicate = PredicateArgs { grep: Some(r"java\..*\.bs".to_string()), ..PredicateArgs::default() }.build().unwrap();

    assert!(predicate.matches(&sample_record(b"java.very.long.bs")));
    assert!(!predicate.matches(&sample_record(b"java.very.long.cs")));
}

#[test]
fn ignore_case_applies_to_fixed_string_and_regex() {
    let fixed = PredicateArgs { fixed_string: Some("error".to_string()), ignore_case: true, ..PredicateArgs::default() }.build().unwrap();
    let regex = PredicateArgs { grep: Some("error".to_string()), ignore_case: true, ..PredicateArgs::default() }.build().unwrap();

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
        fixed_string: Some("hello".to_string()),
        ..PredicateArgs::default()
    }
    .build()
    .unwrap();

    assert!(predicate.matches(&sample_record(b"hello world")));
    assert!(!predicate.matches(&sample_record(b"bye world")));
}

#[test]
fn invalid_regex_is_reported() {
    let error = PredicateArgs { grep: Some("(".to_string()), ..PredicateArgs::default() }.build().unwrap_err();

    assert!(error.to_string().contains("invalid payload matcher"));
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
