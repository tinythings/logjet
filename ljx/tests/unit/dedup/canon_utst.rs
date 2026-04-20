use crate::dedup::DedupGroup;
use crate::dedup::canon::canon_dedup;
use crate::dedup::flat_record::{BucketKey, FlatRecord};

fn make_group(body: &str, service: &str, severity: i32) -> DedupGroup {
    let key = BucketKey { service_name: service.into(), severity_number: severity, scope_name: None, code_filepath: None, code_lineno: None };
    let rec = FlatRecord {
        service_name: service.into(),
        severity_number: severity,
        severity_text: String::new(),
        scope_name: String::new(),
        event_name: String::new(),
        code_filepath: None,
        code_lineno: None,
        trace_id: Vec::new(),
        span_id: Vec::new(),
        time_unix_nano: 100,
        observed_time_unix_nano: 100,
        body: body.into(),
        resource_attrs: Vec::new(),
        scope_attrs: Vec::new(),
        record_attrs: Vec::new(),
    };
    DedupGroup::new(key, 0, rec)
}

#[test]
fn canon_merges_near_duplicate_json() {
    let g1 = make_group(r#"{"type":"actionResponse","requestId":123456,"code":200}"#, "svc", 9);
    let g2 = make_group(r#"{"type":"actionResponse","requestId":789012,"code":200}"#, "svc", 9);
    let result = canon_dedup(vec![g1, g2]);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].count, 2);
    assert_eq!(result[0].body_shape.as_deref(), Some("json"));
    assert!(result[0].canonical_body.is_some());
}

#[test]
fn canon_merges_near_duplicate_freetext() {
    let g1 = make_group("route updated id 48574a68 complete", "svc", 9);
    let g2 = make_group("route updated id 187cc17b complete", "svc", 9);
    let result = canon_dedup(vec![g1, g2]);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].count, 2);
    assert_eq!(result[0].body_shape.as_deref(), Some("freetext"));
}

#[test]
fn canon_merges_near_duplicate_kv() {
    let g1 = make_group("dispatcher=D0110 count=45782 route=48574a68", "svc", 9);
    let g2 = make_group("dispatcher=D0220 count=99001 route=187cc17b", "svc", 9);
    let result = canon_dedup(vec![g1, g2]);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].count, 2);
    assert_eq!(result[0].body_shape.as_deref(), Some("kv"));
}

#[test]
fn canon_merges_source_prefixed() {
    let g1 = make_group("[6354:9548:10080] nav.cpp:465: route updated id 48574a68", "svc", 9);
    let g2 = make_group("[1111:2222:33333] nav.cpp:465: route updated id 187cc17b", "svc", 9);
    let result = canon_dedup(vec![g1, g2]);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].count, 2);
    assert!(result[0].body_shape.as_deref().unwrap().starts_with("prefixed/"));
}

#[test]
fn canon_keeps_different_structures_separate() {
    let g1 = make_group(r#"{"type":"update","code":200}"#, "svc", 9);
    let g2 = make_group(r#"{"type":"error","code":500}"#, "svc", 9);
    let result = canon_dedup(vec![g1, g2]);
    assert_eq!(result.len(), 2);
}

#[test]
fn canon_respects_bucket_boundaries() {
    let g1 = make_group("route updated id 48574a68", "svc-a", 9);
    let g2 = make_group("route updated id 187cc17b", "svc-b", 9);
    let result = canon_dedup(vec![g1, g2]);
    assert_eq!(result.len(), 2);
}

#[test]
fn canon_fewer_groups_than_exact() {
    let groups: Vec<_> = (0..5).map(|i: u64| make_group(&format!("request id={:08x} done", 0xdead0000_u64 + i), "svc", 9)).collect();
    let result = canon_dedup(groups);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].count, 5);
}
