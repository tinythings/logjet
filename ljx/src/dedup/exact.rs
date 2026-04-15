//! Stage 2: exact-hash dedup within each bucket.
//!
//! Computes `xxh3_64(body.as_bytes())` for every record, groups by hash,
//! and collapses identical bodies into `DedupGroup`s. First-seen record
//! is the representative.

use std::collections::HashMap;

use xxhash_rust::xxh3::xxh3_64;

use crate::dedup::DedupGroup;
use crate::dedup::flat_record::{BucketKey, FlatRecord};

/// Exact-dedup all buckets. Returns flattened groups across all buckets.
pub fn dedup(buckets: HashMap<BucketKey, Vec<FlatRecord>>) -> Vec<DedupGroup> {
    let mut all_groups = Vec::new();
    for (key, records) in buckets {
        dedup_bucket(key, records, &mut all_groups);
    }
    all_groups
}

/// Exact-dedup a single bucket's records into groups.
fn dedup_bucket(key: BucketKey, records: Vec<FlatRecord>, out: &mut Vec<DedupGroup>) {
    // Hash → index into `groups` (local to this bucket).
    let mut hash_to_idx: HashMap<u64, usize> = HashMap::new();
    let mut groups: Vec<DedupGroup> = Vec::new();

    for rec in records {
        let hash = xxh3_64(rec.body.as_bytes());
        if let Some(&idx) = hash_to_idx.get(&hash) {
            groups[idx].absorb(&rec);
        } else {
            let idx = groups.len();
            hash_to_idx.insert(hash, idx);
            groups.push(DedupGroup::new(key.clone(), hash, rec));
        }
    }
    out.extend(groups);
}
