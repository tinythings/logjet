//! JSON canonicalisation for log dedup.
//!
//! Walks a `serde_json::Value` tree depth-first, normalising variable
//! parts (large numbers, IDs, paths) while preserving structural keys,
//! small integers, booleans, and null. Output is a deterministic JSON
//! string with sorted keys.

use std::collections::HashSet;
use std::sync::LazyLock;

use serde_json::{Value, json};

/// Keys whose numeric values are always normalised regardless of magnitude.
/// Matched after `normalise_key()` (lowercase, stripped of `_`, `-`, `.`).
static ALWAYS_NORMALISE: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        "requestid",
        "reqid",
        "pathid",
        "routeid",
        "sessionid",
        "correlationid",
        "traceid",
        "spanid",
        "messageid",
        "offset",
        "position",
        "cursor",
        "size",
        "length",
        "len",
        "task",
        "tasks",
        "count",
        "total",
        "num",
        "timestamp",
        "time",
        "epoch",
        "ts",
        "duration",
        "elapsed",
        "latency",
        "port",
        "pid",
        "tid",
        "threadid",
    ])
});

/// Parse a JSON body, canonicalise in-place, serialise with sorted keys.
/// Returns `None` if the body isn't valid JSON.
pub fn canonicalise_json_to_string(body: &str) -> Option<String> {
    let mut val: Value = serde_json::from_str(body).ok()?;
    canonicalise_value("", &mut val);
    Some(sorted_json_string(&val))
}

/// Normalise a key name for denylist lookup: lowercase, strip `_-./`.
pub fn normalise_key(key: &str) -> String {
    key.chars()
        .filter_map(|c| match c {
            '_' | '-' | '.' => None,
            c if c.is_ascii_uppercase() => Some(c.to_ascii_lowercase()),
            c if c.is_ascii_alphanumeric() => Some(c),
            _ => None,
        })
        .collect()
}

/// Recursively canonicalise a JSON value. `parent_key` is the immediate
/// object key (empty at root).
fn canonicalise_value(parent_key: &str, val: &mut Value) {
    match val {
        Value::Object(map) => {
            let keys: Vec<String> = map.keys().cloned().collect();
            for k in keys {
                if let Some(v) = map.get_mut(&k) {
                    canonicalise_value(&k, v);
                }
            }
        }
        Value::Array(arr) => {
            *val = canonicalise_array(arr);
        }
        Value::String(s) => {
            *s = canonicalise_string(s);
        }
        Value::Number(_) => {
            *val = canonicalise_number(parent_key, val);
        }
        // Bool + null: preserve.
        _ => {}
    }
}

/// Classify a JSON string value.
///
/// Short alpha-only strings (enum values, status names) are preserved.
/// Everything that looks like variable data is replaced.
fn canonicalise_string(s: &str) -> String {
    // UUID pattern.
    if s.len() == 36 && looks_like_uuid(s) {
        return "_UUID_".into();
    }

    // Path.
    if s.starts_with('/') {
        return "_PATH_".into();
    }

    // Short alpha-only: enum values, type names, status labels.
    if s.len() <= 12 && s.bytes().all(|b| b.is_ascii_alphabetic() || b == b'_') {
        return s.to_string();
    }

    // Long or contains digits/hex/dots/dashes/slashes → variable.
    if s.len() > 40 || s.bytes().any(|b| b.is_ascii_digit()) || s.contains('.') || s.contains('-') || s.contains('/') {
        return "_".into();
    }

    // Anything else short and clean: keep.
    s.to_string()
}

/// Bounded-value number preservation with key denylist override.
fn canonicalise_number(key: &str, val: &Value) -> Value {
    let norm = normalise_key(key);
    if ALWAYS_NORMALISE.contains(norm.as_str()) {
        return json!(0);
    }

    let Some(n) = val.as_number() else {
        return json!(0);
    };

    // Pure float (not representable as i64/u64).
    if !n.is_i64() && !n.is_u64() {
        if let Some(f) = n.as_f64() {
            return if f == 0.0 || f == 1.0 { json!(f) } else { json!(0.0) };
        }
        return json!(0.0);
    }

    if let Some(i) = n.as_i64() {
        return if i.unsigned_abs() < 1000 { json!(i) } else { json!(0) };
    }
    if let Some(u) = n.as_u64() {
        return if u < 1000 { json!(u) } else { json!(0) };
    }

    json!(0)
}

/// Collapse arrays into shape descriptors.
fn canonicalise_array(arr: &[Value]) -> Value {
    if arr.is_empty() {
        return Value::Array(vec![]);
    }

    let first = &arr[0];

    // All scalars (no objects/arrays)?
    let all_scalar = arr.iter().all(|v| !v.is_object() && !v.is_array());
    if all_scalar {
        return json!({"__arr": "scalar"});
    }

    // First element is object? Extract shape from it.
    if first.is_object()
        && let Some(map) = first.as_object()
    {
        let mut keys: Vec<&String> = map.keys().collect();
        keys.sort();
        let shape: Vec<&str> = keys.iter().map(|k| k.as_str()).collect();
        return json!({"__arr": "object", "__shape": shape});
    }

    // Mixed.
    json!({"__arr": "mixed"})
}

/// Serialise a JSON value with sorted keys for deterministic output.
fn sorted_json_string(val: &Value) -> String {
    match val {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let entries: Vec<String> = keys
                .iter()
                .map(|k| {
                    let v = &map[k.as_str()];
                    format!("{}:{}", serde_json::to_string(k).unwrap(), sorted_json_string(v))
                })
                .collect();
            format!("{{{}}}", entries.join(","))
        }
        Value::Array(arr) => {
            let items: Vec<String> = arr.iter().map(sorted_json_string).collect();
            format!("[{}]", items.join(","))
        }
        _ => serde_json::to_string(val).unwrap(),
    }
}

/// Quick UUID check: 8-4-4-4-12 hex with dashes.
fn looks_like_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b[8] == b'-'
        && b[13] == b'-'
        && b[18] == b'-'
        && b[23] == b'-'
        && b.iter().enumerate().all(|(i, &c)| matches!(i, 8 | 13 | 18 | 23) || c.is_ascii_hexdigit())
}

#[cfg(test)]
#[path = "canon_json_utst.rs"]
mod canon_json_utst;
