//! Stage 3a: detect the shape of a log body.
//!
//! Classification is a short-circuit chain — first match wins.
//! JSON detection caches the parsed `serde_json::Value` to avoid
//! double-parsing in the canonicalisation stage.

use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;

/// Body shape classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyShape {
    /// Valid JSON object or array. Carries the parsed value.
    Json,
    /// Key=value / logfmt style (>= 2 pairs).
    KeyValue,
    /// `[PID:TID:FLAGS] source.cpp:123:` prefix before the real body.
    /// Inner shape and stripped suffix are determined by re-detection.
    SourcePrefixed,
    /// Unstructured free text (catch-all).
    FreeText,
}

/// Detection result: shape + optional cached JSON parse.
pub struct Detected {
    pub shape: BodyShape,
    /// Populated only for `BodyShape::Json`. Cached to avoid double-parse.
    #[allow(dead_code)]
    pub json_value: Option<Value>,
    /// For `SourcePrefixed`: the suffix after stripping the prefix.
    pub stripped_suffix: Option<String>,
}

/// Detect the body shape of a log line.
pub fn detect(body: &str) -> Detected {
    // JSON: starts with an object/array opener and parses successfully.
    let trimmed = body.trim_start();
    if (trimmed.starts_with('{') || trimmed.starts_with('['))
        && let Ok(val) = serde_json::from_str::<Value>(trimmed)
    {
        return Detected { shape: BodyShape::Json, json_value: Some(val), stripped_suffix: None };
    }

    // Prefix + JSON: non-JSON text before a valid JSON object/array payload.
    if let Some(suffix) = strip_json_prefix(body) {
        return Detected { shape: BodyShape::SourcePrefixed, json_value: None, stripped_suffix: Some(suffix) };
    }

    // KeyValue: >= 2 word-boundary key=value pairs.
    if is_key_value(body) {
        return Detected { shape: BodyShape::KeyValue, json_value: None, stripped_suffix: None };
    }

    // SourcePrefixed: [digits:digits:...] source.ext:lineno:
    if let Some(suffix) = strip_source_prefix(body) {
        return Detected { shape: BodyShape::SourcePrefixed, json_value: None, stripped_suffix: Some(suffix) };
    }

    // Catch-all.
    Detected { shape: BodyShape::FreeText, json_value: None, stripped_suffix: None }
}

/// Check for >= 2 key=value pairs with word boundary before the key.
fn is_key_value(body: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"\b\w+=\S+").expect("valid regex"));
    re.find_iter(body).count() >= 2
}

/// Strip `[PID:TID:FLAGS] source.ext:lineno:` prefix, return suffix.
fn strip_source_prefix(body: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"^\[[\d:]+\]\s+\S+\.\w+:\d+:?\s*").expect("valid regex"));
    let m = re.find(body)?;
    let suffix = body[m.end()..].to_string();
    if suffix.is_empty() {
        return None;
    }
    Some(suffix)
}

/// Strip any non-JSON prefix before a valid JSON object/array suffix.
fn strip_json_prefix(body: &str) -> Option<String> {
    let trimmed = body.trim_start();
    let trimmed_start = body.len() - trimmed.len();

    for (idx, ch) in body.char_indices() {
        if !matches!(ch, '{' | '[') {
            continue;
        }
        let suffix = body[idx..].trim_start();
        if !(suffix.starts_with('{') || suffix.starts_with('[')) {
            continue;
        }
        if idx == trimmed_start {
            continue;
        }
        if serde_json::from_str::<Value>(suffix).is_ok() {
            return Some(suffix.to_string());
        }
    }

    None
}

#[cfg(test)]
#[path = "detect_utst.rs"]
mod detect_utst;
