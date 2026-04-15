//! Stage 1: group flat records by bucket key.
//!
//! Produces a `HashMap<BucketKey, Vec<FlatRecord>>`. All downstream dedup
//! is bucket-local — records in different buckets never merge.

use std::collections::HashMap;

use crate::dedup::flat_record::{BucketKey, BucketKeyKind, FlatRecord};

/// Group records into hard-partitioned buckets.
pub fn group(records: Vec<FlatRecord>, kind: BucketKeyKind) -> HashMap<BucketKey, Vec<FlatRecord>> {
    let mut buckets: HashMap<BucketKey, Vec<FlatRecord>> = HashMap::new();
    for rec in records {
        let key = BucketKey::from_record(&rec, kind);
        buckets.entry(key).or_default().push(rec);
    }
    buckets
}
