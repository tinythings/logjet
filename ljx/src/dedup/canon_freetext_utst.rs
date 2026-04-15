use crate::dedup::canon_freetext::{TokenClass, canonicalise_freetext, classify_token};

#[test]
fn uuid() {
    assert_eq!(classify_token("48574a68-40f7-4b2a-9c3d-1234567890ab"), TokenClass::Replace("_UUID_"),);
}

#[test]
fn uuid_uppercase() {
    assert_eq!(classify_token("48574A68-40F7-4B2A-9C3D-1234567890AB"), TokenClass::Replace("_UUID_"),);
}

#[test]
fn ipv6() {
    assert_eq!(classify_token("fd53:7cb8:0383:0000:0000:0000:0000:0001"), TokenClass::Replace("_IPV6_"),);
}

#[test]
fn ipv6_compressed() {
    assert_eq!(classify_token("fd53:7cb8:0383::1:0:0:1"), TokenClass::Replace("_IPV6_"),);
}

#[test]
fn mac_address() {
    assert_eq!(classify_token("fa:00:10:01:86:dd"), TokenClass::Replace("_MAC_"),);
}

#[test]
fn ipv4() {
    assert_eq!(classify_token("192.168.1.1"), TokenClass::Replace("_IPV4_"),);
}

#[test]
fn ipv4_rejects_out_of_range() {
    // 999 > 255 — not a valid IPv4 octet.
    assert_eq!(classify_token("999.999.999.999"), TokenClass::Preserve);
}

#[test]
fn hex_with_prefix() {
    assert_eq!(classify_token("0x005B4F65"), TokenClass::Replace("_HEX_"),);
    assert_eq!(classify_token("0X1200405"), TokenClass::Replace("_HEX_"),);
}

#[test]
fn iso8601_timestamp() {
    assert_eq!(classify_token("2026-03-07T16:53:03"), TokenClass::Replace("_TS_"),);
    assert_eq!(classify_token("2026-03-07T16:53:03.123Z"), TokenClass::Replace("_TS_"),);
}

#[test]
fn file_path() {
    assert_eq!(classify_token("/data/vendor/pathologyArchive/136/VRAMMON"), TokenClass::Replace("_PATH_"),);
}

#[test]
fn file_path_requires_two_slashes() {
    // Single slash + no further slash → not a path.
    assert_eq!(classify_token("/tmp"), TokenClass::Preserve);
}

#[test]
fn long_bare_hex() {
    assert_eq!(classify_token("48574a68"), TokenClass::Replace("_HEX_"),);
    assert_eq!(classify_token("187cc17b"), TokenClass::Replace("_HEX_"),);
}

#[test]
fn long_bare_hex_needs_alpha_hex() {
    // Pure digits with 8+ chars → integer, not hex.
    assert_eq!(classify_token("12345678"), TokenClass::Replace("_N_"),);
}

#[test]
fn large_integer() {
    assert_eq!(classify_token("204035139"), TokenClass::Replace("_N_"));
    assert_eq!(classify_token("465"), TokenClass::Replace("_N_"));
    assert_eq!(classify_token("64400"), TokenClass::Replace("_N_"));
}

#[test]
fn small_integer_preserved() {
    assert_eq!(classify_token("0"), TokenClass::Preserve);
    assert_eq!(classify_token("42"), TokenClass::Preserve);
    assert_eq!(classify_token("99"), TokenClass::Preserve);
}

#[test]
fn float_replaced() {
    assert_eq!(classify_token("0.184"), TokenClass::Replace("_F_"));
    assert_eq!(classify_token("194064.5"), TokenClass::Replace("_F_"));
}

#[test]
fn quoted_string_short_preserved() {
    assert_eq!(classify_token("\"success\""), TokenClass::Preserve);
    assert_eq!(classify_token("'error'"), TokenClass::Preserve);
}

#[test]
fn quoted_string_long_replaced() {
    // Single-word quoted string with inner length > 16.
    assert_eq!(classify_token("\"abcdefghijklmnopqr\""), TokenClass::Replace("\"_\""),);
}

#[test]
fn cpp_symbol_preserved() {
    assert_eq!(classify_token("Dispatcher::AreHorizonWarnings"), TokenClass::Preserve,);
}

#[test]
fn pure_alpha_preserved() {
    assert_eq!(classify_token("ignore"), TokenClass::Preserve);
    assert_eq!(classify_token("Notifying"), TokenClass::Preserve);
    assert_eq!(classify_token("listeners"), TokenClass::Preserve);
}

#[test]
fn empty_string_preserved() {
    assert_eq!(classify_token(""), TokenClass::Preserve);
}

#[test]
fn compound_trip_hex() {
    // "Trip(c3733430)" → Trip(c_HEX_)
    // c3733430 is 8 chars with hex alpha 'c' — wait, 'c' is at start,
    // the alpha frame captures 'T','r','i','p', then '(' is frame,
    // then 'c' is alpha, then '3733430' is digits (7 chars ≥ 3) → _N_.
    // Architecture says: Trip(c_HEX_). But 'c' is scanned as alpha run,
    // '3733430' as digit run (7 digits, no hex alpha) → _N_.
    // The actual result is Trip(c_N_).
    // This matches known weakness J.6 in the architecture doc.
    let result = classify_token("Trip(c3733430)");
    assert_eq!(result, TokenClass::Compound("Trip(c_N_)".to_string()));
}

#[test]
fn compound_t_large_num() {
    // "t17728987830000_c0" → t_N__c0
    // t = alpha, 17728987830000 = 14 digits → _N_, _ = frame, c = alpha, 0 = 1 digit < 3 → preserved.
    let result = classify_token("t17728987830000_c0");
    assert_eq!(result, TokenClass::Compound("t_N__c0".to_string()));
}

#[test]
fn compound_dispatcher_id() {
    // "D0110" → D_N_  (D=alpha, 0110=4 digits ≥ 3)
    let result = classify_token("D0110");
    assert_eq!(result, TokenClass::Compound("D_N_".to_string()));
}

#[test]
fn compound_short_digits_preserved() {
    // "v2" → v2 (alpha + 1 digit < 3 → preserved)
    let result = classify_token("v2");
    assert_eq!(result, TokenClass::Compound("v2".to_string()));
}

#[test]
fn compound_with_hex_run() {
    // "id_0deadbeef0" → id__HEX_
    // id = alpha, _ = frame, 0deadbeef0 = starts with digit → hex branch,
    // 10 chars with hex alpha → _HEX_
    let result = classify_token("id_0deadbeef0");
    assert_eq!(result, TokenClass::Compound("id__HEX_".to_string()));
}

#[test]
fn compound_alpha_hex_ambiguity() {
    // "id_deadbeef00" — 'deadbeef' is scanned as alpha (all a-f are
    // ascii_alphabetic), '00' as 2-digit run (preserved). Known weakness J.6.
    let result = classify_token("id_deadbeef00");
    assert_eq!(result, TokenClass::Compound("id_deadbeef00".to_string()));
}

#[test]
fn full_sentence_mixed() {
    let input = "route progress 192.168.1.1 changed at 2026-03-07T16:53:03 id 48574a68-40f7-4b2a-9c3d-1234567890ab";
    let result = canonicalise_freetext(input);
    assert_eq!(result, "route progress _IPV4_ changed at _TS_ id _UUID_");
}

#[test]
fn full_sentence_preserves_structure() {
    let input = "Notifying listeners route progress has changed";
    let result = canonicalise_freetext(input);
    assert_eq!(result, input);
}

#[test]
fn full_sentence_empty() {
    assert_eq!(canonicalise_freetext(""), "");
}

#[test]
fn full_sentence_single_alpha() {
    assert_eq!(canonicalise_freetext("hello"), "hello");
}

#[test]
fn full_sentence_all_digits() {
    assert_eq!(canonicalise_freetext("42 0 99"), "42 0 99");
    assert_eq!(canonicalise_freetext("12345 999"), "_N_ _N_");
}

#[test]
fn automotive_log_line() {
    let input = "dispatcher=D0110 ignore count=45782 route=48574a68";
    // Not calling canonicalise_freetext directly on KV — this tests
    // individual tokens as free text would see them.
    let result = canonicalise_freetext(input);
    // "dispatcher=D0110" is a compound: dispatcher=D_N_
    // "ignore" is pure alpha
    // "count=45782" is compound: count=_N_
    // "route=48574a68" is compound: route=_HEX_
    assert_eq!(result, "dispatcher=D_N_ ignore count=_N_ route=_HEX_");
}
