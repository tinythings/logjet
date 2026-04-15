use crate::dedup::canon_kv::canonicalise_kv;

#[test]
fn keys_preserved_values_normalised() {
    let input = "worker=F0110 ignore count=45782 route=deadbeef";
    let canon = canonicalise_kv(input);
    assert_eq!(canon, "worker=F_N_ ignore count=_N_ route=_HEX_");
}

#[test]
fn bare_words_classified() {
    let input = "status=ok Placeholder tokens";
    let canon = canonicalise_kv(input);
    assert_eq!(canon, "status=ok Placeholder tokens");
}

#[test]
fn value_with_uuid() {
    let input = "id=aaaaaaaa-bbbb-4ccc-9ddd-eeeeeeeeeeee level=3";
    let canon = canonicalise_kv(input);
    assert_eq!(canon, "id=_UUID_ level=3");
}

#[test]
fn value_with_ip() {
    let input = "host=10.23.45.67 port=8080";
    let canon = canonicalise_kv(input);
    assert_eq!(canon, "host=_IPV4_ port=_N_");
}

#[test]
fn empty_key_treated_as_bare_token() {
    let input = "=weird normal=fine";
    let canon = canonicalise_kv(input);
    // "=weird" has empty key → classified as bare token (compound).
    assert!(canon.contains("normal=fine"));
}

#[test]
fn multiple_equals_in_value() {
    // Split on first '=' only.
    let input = "filter=a=b=c level=5";
    let canon = canonicalise_kv(input);
    // Key is "filter", value is "a=b=c" (compound with alpha).
    assert!(canon.starts_with("filter="));
    assert!(canon.contains("level=5"));
}

#[test]
fn small_numeric_counters_and_ids_are_normalised() {
    let input = "[FakeDispatcher::trace_metric]: agentID=117, max_queue_size_seen=10";
    let canon = canonicalise_kv(input);
    assert_eq!(canon, "[FakeDispatcher::trace_metric]: agentID=_N_, max_queue_size_seen=_N_");
}
