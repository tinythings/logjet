//! Key=value / logfmt canonicalisation.
//!
//! Splits body by whitespace. Tokens containing `=` are treated as
//! key=value pairs: key is preserved, value is normalised via the
//! free-text token classifier. Bare words are also classified.

use crate::dedup::canon_freetext::{TokenClass, classify_token};

/// Canonicalise a key=value body.
pub fn canonicalise_kv(body: &str) -> String {
    body.split_whitespace()
        .map(|token| {
            if let Some((key, val)) = token.split_once('=') {
                if key.is_empty() {
                    return classify_and_render(token);
                }
                let canon_val = classify_and_render(val);
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

#[cfg(test)]
#[path = "canon_kv_utst.rs"]
mod canon_kv_utst;
