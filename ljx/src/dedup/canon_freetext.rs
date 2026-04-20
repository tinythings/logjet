//! Free text token classifier and canonicaliser.
//!
//! Splits a body into whitespace-delimited tokens, classifies each one
//! via char-scanning (no regex soup), and replaces variable tokens with
//! placeholders. The canonical form is the normalised tokens joined by
//! a single space.
//!
//! Also used by KV canonicalisation (values) and source-prefixed (suffix).

use std::sync::OnceLock;

use regex::Regex;

/// Result of classifying a single token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenClass {
    /// Keep the token verbatim.
    Preserve,
    /// Replace the token with a placeholder.
    Replace(&'static str),
    /// The compound mini-lexer produced a mixed result.
    Compound(String),
}

/// Canonicalise a free-text body: split, classify each token, rejoin.
pub fn canonicalise_freetext(body: &str) -> String {
    body.split_whitespace().map(render_token).collect::<Vec<_>>().join(" ")
}

/// Classify a single whitespace-delimited token.
///
/// First match wins. Char-scanning everywhere except ISO8601.
pub fn classify_token(token: &str) -> TokenClass {
    if token.is_empty() {
        return TokenClass::Preserve;
    }

    let bytes = token.as_bytes();
    let len = bytes.len();

    let has_alpha = bytes.iter().any(|b| b.is_ascii_alphabetic());
    let has_digit = bytes.iter().any(|b| b.is_ascii_digit());
    let has_colon = bytes.contains(&b':');
    let has_dot = bytes.contains(&b'.');
    let has_dash = bytes.contains(&b'-');

    // UUID: 8-4-4-4-12 hex with dashes.
    if has_dash && len == 36 && is_uuid(bytes) {
        return TokenClass::Replace("_UUID_");
    }

    // IPv6: 7+ colon-separated hex groups.
    if has_colon && !has_dot && is_ipv6(token) {
        return TokenClass::Replace("_IPV6_");
    }

    // MAC: 6 groups of 2 hex separated by ':'.
    if has_colon && len == 17 && is_mac(bytes) {
        return TokenClass::Replace("_MAC_");
    }

    // IPv4: 4 groups of 1-3 digits separated by '.'.
    if has_dot && !has_alpha && is_ipv4(token) {
        return TokenClass::Replace("_IPV4_");
    }

    // Hex with 0x/0X prefix.
    if len > 2 && (bytes[0] == b'0') && (bytes[1] == b'x' || bytes[1] == b'X') && bytes[2..].iter().all(|b| b.is_ascii_hexdigit()) {
        return TokenClass::Replace("_HEX_");
    }

    // ISO8601 timestamp (only regex in the classifier).
    if has_digit && has_dash && len >= 19 && is_iso8601(token) {
        return TokenClass::Replace("_TS_");
    }

    // File path: starts with '/', 2+ '/' separators.
    if bytes[0] == b'/' && token.matches('/').count() >= 2 {
        return TokenClass::Replace("_PATH_");
    }

    // Long bare hex: 8+ hex chars, nothing else.
    // Pure-digit strings fall through to the integer checks below.
    if len >= 8 && bytes.iter().all(|b| b.is_ascii_hexdigit()) && bytes.iter().any(|b| matches!(b, b'a'..=b'f' | b'A'..=b'F')) {
        return TokenClass::Replace("_HEX_");
    }

    // Quoted string: checked before pure-alpha because quotes lack
    // digits/colons/dots and the shortcut below would swallow them.
    if is_quoted(bytes) {
        let inner_len = len.saturating_sub(2);
        return if inner_len > 16 { TokenClass::Replace("\"_\"") } else { TokenClass::Preserve };
    }

    // Pure alpha (no digits, no colon, no dot). Most common case.
    if !has_digit && has_alpha && !has_colon && !has_dot {
        return TokenClass::Preserve;
    }

    // C++ symbol (contains '::').
    if token.contains("::") {
        return TokenClass::Preserve;
    }

    // Compound token (has alpha + digits): hand off to mini-lexer.
    if has_alpha && has_digit {
        return compound_mini_lexer(token);
    }

    // Pure digits: small (1-2) preserved, large (3+) replaced.
    if bytes.iter().all(|b| b.is_ascii_digit()) {
        return match len {
            0..=2 => TokenClass::Preserve,
            _ => TokenClass::Replace("_N_"),
        };
    }

    // Float: digits.digits.
    if has_dot && is_float(bytes) {
        return TokenClass::Replace("_F_");
    }

    TokenClass::Preserve
}

fn render_token(token: &str) -> String {
    let (core, suffix) = split_trailing_punct(token);
    let rendered = match classify_token(core) {
        TokenClass::Preserve => core.to_string(),
        TokenClass::Replace(r) => r.to_string(),
        TokenClass::Compound(s) => s,
    };
    format!("{rendered}{suffix}")
}

/// Walk char-by-char through a mixed alpha+digit token.
///
/// Preserves alphabetic runs and frame characters. Replaces digit/hex
/// runs based on length. Produces a single canonical string.
fn compound_mini_lexer(token: &str) -> TokenClass {
    let mut out = String::with_capacity(token.len());
    let bytes = token.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        let b = bytes[i];

        if is_frame_char(b) {
            out.push(b as char);
            i += 1;
        } else if b.is_ascii_hexdigit() {
            let start = i;
            let mut has_hex_alpha = false;
            let mut has_digit = false;
            while i < len && bytes[i].is_ascii_hexdigit() {
                if matches!(bytes[i], b'a'..=b'f' | b'A'..=b'F') {
                    has_hex_alpha = true;
                }
                if bytes[i].is_ascii_digit() {
                    has_digit = true;
                }
                i += 1;
            }
            let run_len = i - start;

            if has_hex_alpha && has_digit && run_len >= 8 {
                out.push_str("_HEX_");
            } else if b.is_ascii_alphabetic() {
                i = start;
                while i < len && bytes[i].is_ascii_alphabetic() {
                    i += 1;
                }
                out.push_str(&token[start..i]);
            } else if run_len >= 3 {
                out.push_str("_N_");
            } else {
                out.push_str(&token[start..i]);
            }
        } else if b.is_ascii_alphabetic() {
            let start = i;
            while i < len && bytes[i].is_ascii_alphabetic() {
                i += 1;
            }
            out.push_str(&token[start..i]);
        } else {
            out.push(b as char);
            i += 1;
        }
    }

    TokenClass::Compound(out)
}

/// Frame characters preserved verbatim by the mini-lexer.
fn is_frame_char(b: u8) -> bool {
    matches!(b, b'(' | b')' | b'[' | b']' | b'{' | b'}' | b'#' | b'=' | b'-' | b'.' | b',' | b':' | b'/' | b'@' | b'_')
}

fn split_trailing_punct(token: &str) -> (&str, &str) {
    let end = token.trim_end_matches([',', ';', ':', ')', ']', '}']).len();
    token.split_at(end)
}

/// UUID: exactly 8-4-4-4-12 hex chars with dashes.
fn is_uuid(b: &[u8]) -> bool {
    if b.len() != 36 {
        return false;
    }
    b[8] == b'-'
        && b[13] == b'-'
        && b[18] == b'-'
        && b[23] == b'-'
        && b.iter().enumerate().all(|(i, &c)| if i == 8 || i == 13 || i == 18 || i == 23 { true } else { c.is_ascii_hexdigit() })
}

/// IPv6: 7+ groups of hex separated by ':'.
fn is_ipv6(token: &str) -> bool {
    let groups: Vec<&str> = token.split(':').collect();
    if groups.len() < 7 {
        return false;
    }
    groups.iter().all(|g| g.is_empty() || (g.len() <= 4 && g.bytes().all(|b| b.is_ascii_hexdigit())))
}

/// MAC address: 6 groups of exactly 2 hex chars separated by ':'.
fn is_mac(b: &[u8]) -> bool {
    if b.len() != 17 {
        return false;
    }
    for i in 0..6 {
        let base = i * 3;
        if !b[base].is_ascii_hexdigit() || !b[base + 1].is_ascii_hexdigit() {
            return false;
        }
        if i < 5 && b[base + 2] != b':' {
            return false;
        }
    }
    true
}

/// IPv4: exactly 4 groups of 1-3 digits separated by '.'.
fn is_ipv4(token: &str) -> bool {
    let groups: Vec<&str> = token.split('.').collect();
    if groups.len() != 4 {
        return false;
    }
    groups.iter().all(|g| !g.is_empty() && g.len() <= 3 && g.bytes().all(|b| b.is_ascii_digit()) && g.parse::<u16>().is_ok_and(|n| n <= 255))
}

/// ISO8601 timestamp. Only regex in the whole classifier.
fn is_iso8601(token: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"^\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}").expect("valid regex"));
    re.is_match(token)
}

/// Float: one or more digits, one dot, one or more digits.
fn is_float(b: &[u8]) -> bool {
    let mut dot_pos = None;
    for (i, &c) in b.iter().enumerate() {
        if c == b'.' {
            if dot_pos.is_some() {
                return false;
            }
            dot_pos = Some(i);
        } else if !c.is_ascii_digit() {
            return false;
        }
    }
    match dot_pos {
        Some(p) => p > 0 && p < b.len() - 1,
        None => false,
    }
}

/// Quoted string: starts and ends with matching `'` or `"`.
fn is_quoted(b: &[u8]) -> bool {
    if b.len() < 2 {
        return false;
    }
    (b[0] == b'"' && b[b.len() - 1] == b'"') || (b[0] == b'\'' && b[b.len() - 1] == b'\'')
}

#[cfg(test)]
#[path = "../../tests/unit/dedup/canon_freetext_utst.rs"]
mod canon_freetext_utst;
