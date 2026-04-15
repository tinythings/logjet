use crate::dedup::detect::{BodyShape, detect};

#[test]
fn json_object() {
    let d = detect(r#"{"type":"actionResponse","code":200}"#);
    assert_eq!(d.shape, BodyShape::Json);
    assert!(d.json_value.is_some());
}

#[test]
fn json_array() {
    let d = detect(r#"[{"name":"a"},{"name":"b"}]"#);
    assert_eq!(d.shape, BodyShape::Json);
    assert!(d.json_value.is_some());
}

#[test]
fn json_with_leading_whitespace() {
    let d = detect(r#"  {"key":"val"}"#);
    assert_eq!(d.shape, BodyShape::Json);
}

#[test]
fn invalid_json_falls_through() {
    let d = detect(r#"{broken json"#);
    assert_ne!(d.shape, BodyShape::Json);
}

#[test]
fn key_value_two_pairs() {
    let d = detect("dispatcher=D0110 ignore=true level=3");
    assert_eq!(d.shape, BodyShape::KeyValue);
}

#[test]
fn key_value_needs_two_pairs() {
    let d = detect("single=pair only text here");
    assert_eq!(d.shape, BodyShape::FreeText);
}

#[test]
fn source_prefixed() {
    let d = detect("[6354:9548:10080] leg_progress_calculator.cpp:465: route updated");
    assert_eq!(d.shape, BodyShape::SourcePrefixed);
    assert_eq!(d.stripped_suffix.as_deref(), Some("route updated"));
}

#[test]
fn source_prefixed_with_json_suffix() {
    let d = detect(r#"[123:456:789] nav.cpp:42: {"type":"update"}"#);
    assert_eq!(d.shape, BodyShape::SourcePrefixed);
    let suffix = d.stripped_suffix.unwrap();
    assert!(suffix.starts_with('{'));
}

#[test]
fn source_prefixed_empty_suffix_falls_through() {
    // Only prefix, no actual body content after it.
    let d = detect("[123:456] foo.cpp:1: ");
    assert_ne!(d.shape, BodyShape::SourcePrefixed);
}

#[test]
fn free_text_catchall() {
    let d = detect("Notifying listeners route progress has changed");
    assert_eq!(d.shape, BodyShape::FreeText);
}

#[test]
fn json_takes_priority_over_kv() {
    // JSON with key=value-like content inside still detected as JSON.
    let d = detect(r#"{"key=val":"foo","another=thing":"bar"}"#);
    assert_eq!(d.shape, BodyShape::Json);
}
