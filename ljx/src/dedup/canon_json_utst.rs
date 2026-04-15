use serde_json::json;

use crate::dedup::canon_json::{canonicalise_json_to_string, normalise_key};

// normalise_key

#[test]
fn normalise_key_camel_case() {
    assert_eq!(normalise_key("requestId"), "requestid");
}

#[test]
fn normalise_key_snake_case() {
    assert_eq!(normalise_key("request_id"), "requestid");
}

#[test]
fn normalise_key_screaming_snake() {
    assert_eq!(normalise_key("REQUEST_ID"), "requestid");
}

#[test]
fn normalise_key_dotted() {
    assert_eq!(normalise_key("trace.id"), "traceid");
}

#[test]
fn normalise_key_no_substring_match() {
    // "timeout" normalises to "timeout", not "time" — exact match only.
    assert_eq!(normalise_key("timeout"), "timeout");
    assert_eq!(normalise_key("important"), "important");
}

// String values

#[test]
fn short_alpha_preserved() {
    let input = r#"{"kind":"fakeResponse","status":"placeholder"}"#;
    let canon = canonicalise_json_to_string(input).unwrap();
    assert!(canon.contains("\"fakeResponse\""));
    assert!(canon.contains("\"placeholder\""));
}

#[test]
fn long_string_replaced() {
    let input = r#"{"msg":"this is a very long placeholder string that exceeds forty chars"}"#;
    let canon = canonicalise_json_to_string(input).unwrap();
    assert!(canon.contains("\"_\""));
}

#[test]
fn string_with_digits_replaced() {
    let input = r#"{"id":"fake123value"}"#;
    let canon = canonicalise_json_to_string(input).unwrap();
    assert!(canon.contains("\"_\""));
}

#[test]
fn uuid_string_replaced() {
    let input = r#"{"trace":"aaaaaaaa-bbbb-4ccc-9ddd-eeeeeeeeeeee"}"#;
    let canon = canonicalise_json_to_string(input).unwrap();
    assert!(canon.contains("\"_UUID_\""));
}

#[test]
fn path_string_replaced() {
    let input = r#"{"file":"/fake/root/archive"}"#;
    let canon = canonicalise_json_to_string(input).unwrap();
    assert!(canon.contains("\"_PATH_\""));
}

// Number values

#[test]
fn small_integer_preserved() {
    let input = r#"{"statusCode":200,"gear":3}"#;
    let canon = canonicalise_json_to_string(input).unwrap();
    assert!(canon.contains("200"));
    assert!(canon.contains("3"));
}

#[test]
fn large_integer_replaced() {
    let input = r#"{"offset":123456}"#;
    let canon = canonicalise_json_to_string(input).unwrap();
    // "offset" is in the denylist, so replaced regardless.
    // But even without denylist, 123456 >= 1000 → 0.
    assert!(canon.contains(":0"));
}

#[test]
fn negative_small_preserved() {
    let input = r#"{"code":-42}"#;
    let canon = canonicalise_json_to_string(input).unwrap();
    assert!(canon.contains("-42"));
}

#[test]
fn negative_large_replaced() {
    let input = r#"{"delta":-5000}"#;
    let canon = canonicalise_json_to_string(input).unwrap();
    assert!(canon.contains(":0"));
}

#[test]
fn float_replaced() {
    let input = r#"{"latency":0.184}"#;
    let canon = canonicalise_json_to_string(input).unwrap();
    // "latency" is in denylist → 0. Even without denylist, float → 0.0.
    assert!(canon.contains(":0"));
}

#[test]
fn float_zero_and_one_preserved() {
    let input = r#"{"a":0.0,"b":1.0}"#;
    let canon = canonicalise_json_to_string(input).unwrap();
    assert!(canon.contains("0.0"));
    assert!(canon.contains("1.0"));
}

// Key denylist

#[test]
fn denylist_overrides_small_number() {
    // "requestId" is in denylist → always normalised, even value=5.
    let input = r#"{"requestId":5}"#;
    let canon = canonicalise_json_to_string(input).unwrap();
    assert!(canon.contains(":0"));
    assert!(!canon.contains(":5"));
}

#[test]
fn non_denylist_small_number_preserved() {
    // "statusCode" is NOT in denylist. 200 < 1000 → preserved.
    let input = r#"{"statusCode":200}"#;
    let canon = canonicalise_json_to_string(input).unwrap();
    assert!(canon.contains("200"));
}

#[test]
fn denylist_with_underscore_key() {
    // "request_id" normalises to "requestid" → in denylist.
    let input = r#"{"request_id":7}"#;
    let canon = canonicalise_json_to_string(input).unwrap();
    assert!(canon.contains(":0"));
}

#[test]
fn tasks_counter_is_normalised() {
    let input = r#"{"method":"fakeAction","msg":"No change in placeholder","tasks":8}"#;
    let canon = canonicalise_json_to_string(input).unwrap();
    assert!(canon.contains("\"tasks\":0"));
    assert!(!canon.contains("\"tasks\":8"));
}

// Bool + null

#[test]
fn bool_and_null_preserved() {
    let input = r#"{"flag":true,"other":false,"empty":null}"#;
    let canon = canonicalise_json_to_string(input).unwrap();
    assert!(canon.contains("true"));
    assert!(canon.contains("false"));
    assert!(canon.contains("null"));
}

// Arrays

#[test]
fn empty_array_preserved() {
    let input = r#"{"items":[]}"#;
    let canon = canonicalise_json_to_string(input).unwrap();
    assert!(canon.contains("[]"));
}

#[test]
fn scalar_array_collapsed() {
    let input = r#"{"ids":[1,2,3,4,5]}"#;
    let canon = canonicalise_json_to_string(input).unwrap();
    assert!(canon.contains("__arr"));
    assert!(canon.contains("scalar"));
}

#[test]
fn object_array_collapsed_with_shape() {
    let input = r#"{"items":[{"name":"fake_a","value":1},{"name":"fake_b","value":2}]}"#;
    let canon = canonicalise_json_to_string(input).unwrap();
    assert!(canon.contains("__arr"));
    assert!(canon.contains("object"));
    assert!(canon.contains("__shape"));
    assert!(canon.contains("name"));
    assert!(canon.contains("value"));
}

#[test]
fn mixed_array_collapsed() {
    let input = r#"{"data":[1,{"key":"fake"}]}"#;
    let canon = canonicalise_json_to_string(input).unwrap();
    assert!(canon.contains("mixed"));
}

// Sorted keys

#[test]
fn sorted_keys_deterministic() {
    let a = r#"{"z":1,"a":2,"m":3}"#;
    let b = r#"{"a":2,"m":3,"z":1}"#;
    let canon_a = canonicalise_json_to_string(a).unwrap();
    let canon_b = canonicalise_json_to_string(b).unwrap();
    assert_eq!(canon_a, canon_b);
    // "a" should come before "m" before "z".
    let pos_a = canon_a.find("\"a\"").unwrap();
    let pos_m = canon_a.find("\"m\"").unwrap();
    let pos_z = canon_a.find("\"z\"").unwrap();
    assert!(pos_a < pos_m);
    assert!(pos_m < pos_z);
}

// Nested objects: immediate parent key used

#[test]
fn nested_denylist_uses_immediate_key() {
    // "response.time" — the immediate key is "time" → in denylist.
    let input = r#"{"response":{"time":42}}"#;
    let canon = canonicalise_json_to_string(input).unwrap();
    // 42 < 1000 but "time" is in denylist → normalised.
    assert!(!canon.contains("42"));
}

// Structure preservation

#[test]
fn keys_never_touched() {
    let input = r#"{"my_special_key":"fake","Another-Key":123}"#;
    let canon = canonicalise_json_to_string(input).unwrap();
    assert!(canon.contains("my_special_key"));
    assert!(canon.contains("Another-Key"));
}

// Invalid JSON returns None

#[test]
fn invalid_json_returns_none() {
    assert!(canonicalise_json_to_string("plain placeholder text").is_none());
    assert!(canonicalise_json_to_string("{broken").is_none());
}

// Full round-trip

#[test]
fn automotive_json_body() {
    let input = json!({
        "type": "fakeResponse",
        "requestId": 4_917_575_253_552_093_804_u64,
        "actionId": "aaaaaaaa-bbbb-4ccc-9ddd-eeeeeeeeeeee",
        "statusCode": 200,
        "items": [{"name": "fake_route", "distance": 14500}],
        "active": true,
        "error": null
    });
    let canon = canonicalise_json_to_string(&input.to_string()).unwrap();

    // Keys preserved.
    assert!(canon.contains("\"type\""));
    assert!(canon.contains("\"requestId\""));
    assert!(canon.contains("\"statusCode\""));

    // "type": "fakeResponse" — short alpha → preserved.
    assert!(canon.contains("\"fakeResponse\""));

    // "requestId" in denylist → 0.
    // "statusCode" 200 < 1000, not in denylist → preserved.
    assert!(canon.contains("\"statusCode\":200"));

    // UUID replaced.
    assert!(canon.contains("\"_UUID_\""));

    // Bool + null preserved.
    assert!(canon.contains("true"));
    assert!(canon.contains("null"));

    // Array collapsed to object shape.
    assert!(canon.contains("__arr"));
}
