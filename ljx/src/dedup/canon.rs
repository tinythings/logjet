//! Stage 3 orchestrator: detect body shape → canonicalise → hash → merge.
//!
//! Takes exact-dedup groups, canonicalises each representative body,
//! hashes the canonical form, and merges groups within the same bucket
//! that share the same canonical hash.

use std::collections::HashMap;

use xxhash_rust::xxh3::xxh3_64;

use crate::dedup::DedupGroup;
use crate::dedup::canon_freetext::canonicalise_freetext;
use crate::dedup::canon_json::canonicalise_json_to_string;
use crate::dedup::canon_kv::canonicalise_kv;
use crate::dedup::detect::{BodyShape, detect};

/// Canonicalise and merge groups. Groups in different buckets never merge.
pub fn canon_dedup(mut groups: Vec<DedupGroup>) -> Vec<DedupGroup> {
    // Annotate each group with its canonical body and shape label.
    for g in &mut groups {
        let (canon, shape) = canonicalise_body(&g.representative.body);
        g.canonical_body = Some(canon);
        g.body_shape = Some(shape);
    }

    // Merge groups within the same bucket sharing the same canonical hash.
    // Key: (bucket_key, canon_hash) → index in output vec.
    let mut merged: Vec<DedupGroup> = Vec::with_capacity(groups.len());
    let mut index: HashMap<(crate::dedup::flat_record::BucketKey, u64), usize> = HashMap::new();

    for g in groups {
        let canon_hash = xxh3_64(g.canonical_body.as_deref().unwrap_or("").as_bytes());
        let merge_key = (g.bucket_key.clone(), canon_hash);

        if let Some(&idx) = index.get(&merge_key) {
            merged[idx].merge_group(g);
        } else {
            let idx = merged.len();
            let mut g = g;
            g.signature = canon_hash;
            index.insert(merge_key, idx);
            merged.push(g);
        }
    }

    merged
}

/// Canonicalise a body string, returning (canonical_form, shape_label).
fn canonicalise_body(body: &str) -> (String, String) {
    let detected = detect(body);
    match detected.shape {
        BodyShape::Json => {
            if let Some(canon) = canonicalise_json_to_string(body.trim_start()) {
                (canon, "json".into())
            } else {
                // JSON detection succeeded but canonicalisation failed (shouldn't
                // happen, but fall back to free text).
                (canonicalise_freetext(body), "freetext".into())
            }
        }
        BodyShape::KeyValue => (canonicalise_kv(body), "kv".into()),
        BodyShape::SourcePrefixed => {
            let suffix = detected.stripped_suffix.as_deref().unwrap_or(body);
            let (canon, inner_shape) = canonicalise_body(suffix);
            (canon, format!("prefixed/{inner_shape}"))
        }
        BodyShape::FreeText => (canonicalise_freetext(body), "freetext".into()),
    }
}

#[cfg(test)]
#[path = "canon_utst.rs"]
mod canon_utst;
