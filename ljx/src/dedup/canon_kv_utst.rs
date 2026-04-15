use crate::dedup::canon_kv::canonicalise_kv;

#[test]
fn keys_preserved_values_normalised() {
    let input = "dispatcher=D0110 ignore count=45782 route=48574a68";
    let canon = canonicalise_kv(input);
    assert_eq!(canon, "dispatcher=D_N_ ignore count=_N_ route=_HEX_");
}

#[test]
fn bare_words_classified() {
    let input = "status=ok Notifying listeners";
    let canon = canonicalise_kv(input);
    assert_eq!(canon, "status=ok Notifying listeners");
}

#[test]
fn value_with_uuid() {
    let input = "id=48574a68-40f7-4b2a-9c3d-1234567890ab level=3";
    let canon = canonicalise_kv(input);
    assert_eq!(canon, "id=_UUID_ level=3");
}

#[test]
fn value_with_ip() {
    let input = "host=192.168.1.1 port=8080";
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
