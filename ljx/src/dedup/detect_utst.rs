use crate::dedup::detect::{BodyShape, detect};

#[test]
fn json_object() {
    let d = detect(r#"{"kind":"fake_event","code":200}"#);
    assert_eq!(d.shape, BodyShape::Json);
    assert!(d.json_value.is_some());
}

#[test]
fn json_array() {
    let d = detect(r#"[{"label":"fake_a"},{"label":"fake_b"}]"#);
    assert_eq!(d.shape, BodyShape::Json);
    assert!(d.json_value.is_some());
}

#[test]
fn json_with_leading_whitespace() {
    let d = detect(r#"  {"fake_key":"fake_value"}"#);
    assert_eq!(d.shape, BodyShape::Json);
}

#[test]
fn invalid_json_falls_through() {
    let d = detect(r#"{broken json"#);
    assert_ne!(d.shape, BodyShape::Json);
}

#[test]
fn key_value_two_pairs() {
    let d = detect("worker=FAKE01 enabled=true level=3");
    assert_eq!(d.shape, BodyShape::KeyValue);
}

#[test]
fn key_value_needs_two_pairs() {
    let d = detect("single=fake only placeholder text");
    assert_eq!(d.shape, BodyShape::FreeText);
}

#[test]
fn source_prefixed() {
    let d = detect("[111:222:333] fake_component.cpp:77: placeholder updated");
    assert_eq!(d.shape, BodyShape::SourcePrefixed);
    assert_eq!(d.stripped_suffix.as_deref(), Some("placeholder updated"));
}

#[test]
fn source_prefixed_with_json_suffix() {
    let d = detect(r#"[111:222:333] fake_nav.cpp:42: {"kind":"fake_update"}"#);
    assert_eq!(d.shape, BodyShape::SourcePrefixed);
    let suffix = d.stripped_suffix.unwrap();
    assert!(suffix.starts_with('{'));
}

#[test]
fn generic_prefix_with_json_suffix() {
    let d = detect(r#"fake prefix: {"requestId":22,"kind":"fake_response","statusCode":299}"#);
    assert_eq!(d.shape, BodyShape::SourcePrefixed);
    let suffix = d.stripped_suffix.unwrap();
    assert!(suffix.starts_with('{'));
    assert!(suffix.contains("\"requestId\":22"));
}

#[test]
fn bracketed_prefix_with_json_suffix() {
    let d = detect(r#"[111:222:333] {"requestId":0,"kind":"fake_response","statusCode":299}"#);
    assert_eq!(d.shape, BodyShape::SourcePrefixed);
    let suffix = d.stripped_suffix.unwrap();
    assert!(suffix.starts_with('{'));
    assert!(suffix.contains("\"requestId\":0"));
}

#[test]
fn source_prefixed_empty_suffix_falls_through() {
    // Only prefix, no actual body content after it.
    let d = detect("[111:222] fake.cpp:1: ");
    assert_ne!(d.shape, BodyShape::SourcePrefixed);
}

#[test]
fn free_text_catchall() {
    let d = detect("Placeholder words in a totally fake sentence");
    assert_eq!(d.shape, BodyShape::FreeText);
}

#[test]
fn json_takes_priority_over_kv() {
    // JSON with key=value-like content inside still detected as JSON.
    let d = detect(r#"{"fake=key":"alpha","other=thing":"beta"}"#);
    assert_eq!(d.shape, BodyShape::Json);
}
