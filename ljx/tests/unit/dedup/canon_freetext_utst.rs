use crate::dedup::canon_freetext::{TokenClass, canonicalise_freetext, classify_token};

#[test]
fn uuid() {
    assert_eq!(classify_token("aaaaaaaa-bbbb-4ccc-9ddd-eeeeeeeeeeee"), TokenClass::Replace("_UUID_"),);
}

#[test]
fn uuid_uppercase() {
    assert_eq!(classify_token("AAAAAAAA-BBBB-4CCC-9DDD-EEEEEEEEEEEE"), TokenClass::Replace("_UUID_"),);
}

#[test]
fn ipv6() {
    assert_eq!(classify_token("fd00:beef:0383:0000:0000:0000:0000:0001"), TokenClass::Replace("_IPV6_"),);
}

#[test]
fn ipv6_compressed() {
    assert_eq!(classify_token("fd00:beef:0383::1:0:0:1"), TokenClass::Replace("_IPV6_"),);
}

#[test]
fn mac_address() {
    assert_eq!(classify_token("fa:00:10:01:86:dd"), TokenClass::Replace("_MAC_"),);
}

#[test]
fn ipv4() {
    assert_eq!(classify_token("10.23.45.67"), TokenClass::Replace("_IPV4_"),);
}

#[test]
fn ipv4_rejects_out_of_range() {
    assert_eq!(classify_token("999.999.999.999"), TokenClass::Preserve);
}

#[test]
fn hex_with_prefix() {
    assert_eq!(classify_token("0x005B4F65"), TokenClass::Replace("_HEX_"),);
    assert_eq!(classify_token("0X1200405"), TokenClass::Replace("_HEX_"),);
}

#[test]
fn iso8601_timestamp() {
    assert_eq!(classify_token("2030-01-02T03:04:05"), TokenClass::Replace("_TS_"),);
    assert_eq!(classify_token("2030-01-02T03:04:05.123Z"), TokenClass::Replace("_TS_"),);
}

#[test]
fn file_path() {
    assert_eq!(classify_token("/fake/root/archive/123/PLACEHOLDER"), TokenClass::Replace("_PATH_"),);
}

#[test]
fn file_path_requires_two_slashes() {
    assert_eq!(classify_token("/tmp"), TokenClass::Preserve);
}

#[test]
fn long_bare_hex() {
    assert_eq!(classify_token("48574a68"), TokenClass::Replace("_HEX_"),);
    assert_eq!(classify_token("187cc17b"), TokenClass::Replace("_HEX_"),);
}

#[test]
fn long_bare_hex_needs_alpha_hex() {
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
fn float_with_trailing_comma_replaced_in_sentence() {
    let input = "fakeAction failed for metric: 12.940000, key: fake_route_1";
    let result = canonicalise_freetext(input);
    assert_eq!(result, "fakeAction failed for metric: _F_, key: fake_route_1");
}

#[test]
fn quoted_string_short_preserved() {
    assert_eq!(classify_token("\"success\""), TokenClass::Preserve);
    assert_eq!(classify_token("'error'"), TokenClass::Preserve);
}

#[test]
fn quoted_string_long_replaced() {
    assert_eq!(classify_token("\"abcdefghijklmnopqr\""), TokenClass::Replace("\"_\""),);
}

#[test]
fn cpp_symbol_preserved() {
    assert_eq!(classify_token("Placeholder::CheckSignals"), TokenClass::Preserve,);
}

#[test]
fn pure_alpha_preserved() {
    assert_eq!(classify_token("ignore"), TokenClass::Preserve);
    assert_eq!(classify_token("Placeholder"), TokenClass::Preserve);
    assert_eq!(classify_token("tokens"), TokenClass::Preserve);
}

#[test]
fn empty_string_preserved() {
    assert_eq!(classify_token(""), TokenClass::Preserve);
}

#[test]
fn compound_trip_hex() {
    let result = classify_token("Fake(c1234abc)");
    assert_eq!(result, TokenClass::Compound("Fake(_HEX_)".to_string()));
}

#[test]
fn compound_t_large_num() {
    let result = classify_token("x17728987830000_c0");
    assert_eq!(result, TokenClass::Compound("x_N__c0".to_string()));
}

#[test]
fn compound_dispatcher_id() {
    let result = classify_token("F0110");
    assert_eq!(result, TokenClass::Compound("F_N_".to_string()));
}

#[test]
fn compound_short_digits_preserved() {
    let result = classify_token("v2");
    assert_eq!(result, TokenClass::Compound("v2".to_string()));
}

#[test]
fn compound_with_hex_run() {
    let result = classify_token("id_0deadbeef0");
    assert_eq!(result, TokenClass::Compound("id__HEX_".to_string()));
}

#[test]
fn compound_alpha_hex_ambiguity() {
    let result = classify_token("id_deadbeef00");
    assert_eq!(result, TokenClass::Compound("id__HEX_".to_string()));
}

#[test]
fn full_sentence_mixed() {
    let input = "fake progress 10.23.45.67 changed at 2030-01-02T03:04:05 id aaaaaaaa-bbbb-4ccc-9ddd-eeeeeeeeeeee";
    let result = canonicalise_freetext(input);
    assert_eq!(result, "fake progress _IPV4_ changed at _TS_ id _UUID_");
}

#[test]
fn full_sentence_preserves_structure() {
    let input = "Placeholder tokens fake progress has changed";
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
    let input = "worker=F0110 ignore count=45782 route=c0ffee12";
    let result = canonicalise_freetext(input);
    assert_eq!(result, "worker=F_N_ ignore count=_N_ route=_HEX_");
}
