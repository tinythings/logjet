//! Key=value / logfmt canonicalisation.
//!
//! Splits body by whitespace. Tokens containing `=` are treated as
//! key=value pairs: key is preserved, value is normalised via the
//! free-text token classifier. Bare words are also classified.

use std::collections::HashSet;
use std::sync::LazyLock;

use crate::dedup::canon_freetext::{TokenClass, classify_token};

/// KV keys whose values are treated as variable counters / IDs even when small.
static ALWAYS_NORMALISE_VALUE: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        "agentid",
        "requestid",
        "reqid",
        "traceid",
        "spanid",
        "messageid",
        "task",
        "tasks",
        "count",
        "total",
        "num",
        "size",
        "queuesize",
        "maxqueuesize",
        "maxqueuesizeseen",
    ])
});

/// Canonicalise a key=value body.
pub fn canonicalise_kv(body: &str) -> String {
    body.split_whitespace()
        .map(|token| {
            if let Some((key, val)) = token.split_once('=') {
                if key.is_empty() {
                    return classify_and_render(token);
                }
                let canon_val = canonicalise_value_for_key(key, val);
                format!("{key}={canon_val}")
            } else {
                classify_and_render(token)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn classify_and_render(token: &str) -> String {
    match classify_token(token) {
        TokenClass::Preserve => token.to_string(),
        TokenClass::Replace(r) => r.to_string(),
        TokenClass::Compound(s) => s,
    }
}

fn canonicalise_value_for_key(key: &str, val: &str) -> String {
    let (core, suffix) = split_trailing_punct(val);
    if core.is_empty() {
        return val.to_string();
    }

    let rendered = if ALWAYS_NORMALISE_VALUE.contains(normalise_key(key).as_str()) && looks_numericish(core) {
        if core.contains('.') { "_F_".to_string() } else { "_N_".to_string() }
    } else {
        classify_and_render(core)
    };

    format!("{rendered}{suffix}")
}

fn normalise_key(key: &str) -> String {
    key.chars()
        .filter_map(|c| match c {
            '_' | '-' | '.' => None,
            c if c.is_ascii_uppercase() => Some(c.to_ascii_lowercase()),
            c if c.is_ascii_alphanumeric() => Some(c),
            _ => None,
        })
        .collect()
}

fn split_trailing_punct(token: &str) -> (&str, &str) {
    let end = token.trim_end_matches(|c: char| matches!(c, ',' | ';' | ':' | ')' | ']' | '}')).len();
    token.split_at(end)
}

fn looks_numericish(token: &str) -> bool {
    token.bytes().all(|b| b.is_ascii_digit()) || token.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'-'))
}

#[cfg(test)]
#[path = "canon_kv_utst.rs"]
mod canon_kv_utst;
