//! Local / burst-oriented deduplication.
//!
//! This mode preserves ordering and only merges records into the current
//! trailing group when they belong to the same bucket and match the active
//! matcher level. Once a different record starts a new run, later repeats
//! become new groups.

use xxhash_rust::xxh3::xxh3_64;

use crate::dedup::canon::canonicalise_body;
use crate::dedup::drain3::{Drain, DrainConfig};
use crate::dedup::flat_record::{BucketKey, FlatRecord};
use crate::dedup::{DedupGroup, DedupMatchMode, DedupOpts};

pub fn dedup(records: Vec<FlatRecord>, opts: &DedupOpts) -> Vec<DedupGroup> {
    let mut groups: Vec<DedupGroup> = Vec::new();

    for rec in records {
        let bucket_key = BucketKey::from_record(&rec, opts.bucket_key);
        let candidate = CollapseCandidate::from_record(rec, bucket_key, opts.match_mode);

        if let Some(last) = groups.last_mut()
            && last.bucket_key == candidate.bucket_key
            && (same_signature(last, &candidate) || (opts.match_mode == DedupMatchMode::Full && drain_matches(last, &candidate, opts)))
        {
            merge_into_group(last, candidate);
            continue;
        }

        groups.push(candidate.into_group());
    }

    groups
}

struct CollapseCandidate {
    bucket_key: BucketKey,
    signature: u64,
    canonical_body: Option<String>,
    body_shape: Option<String>,
    record: FlatRecord,
}

impl CollapseCandidate {
    fn from_record(record: FlatRecord, bucket_key: BucketKey, match_mode: DedupMatchMode) -> Self {
        match match_mode {
            DedupMatchMode::Exact => {
                let signature = xxh3_64(record.body.as_bytes());
                Self { bucket_key, signature, canonical_body: None, body_shape: None, record }
            }
            DedupMatchMode::Hash2 | DedupMatchMode::Full => {
                let (canonical_body, body_shape) = canonicalise_body(&record.body);
                let signature = xxh3_64(canonical_body.as_bytes());
                Self { bucket_key, signature, canonical_body: Some(canonical_body), body_shape: Some(body_shape), record }
            }
        }
    }

    fn into_group(self) -> DedupGroup {
        let mut group = DedupGroup::new(self.bucket_key, self.signature, self.record);
        group.canonical_body = self.canonical_body;
        group.body_shape = self.body_shape;
        group
    }
}

fn same_signature(group: &DedupGroup, candidate: &CollapseCandidate) -> bool {
    group.signature == candidate.signature
}

fn merge_into_group(group: &mut DedupGroup, candidate: CollapseCandidate) {
    group.absorb(&candidate.record);
}

fn drain_matches(group: &mut DedupGroup, candidate: &CollapseCandidate, opts: &DedupOpts) -> bool {
    let left = group.drain3_template.as_deref().or(group.canonical_body.as_deref()).unwrap_or(&group.representative.body);
    let right = candidate.canonical_body.as_deref().unwrap_or(&candidate.record.body);

    let cfg = DrainConfig {
        depth: opts.drain.depth,
        sim_th: opts.drain.sim_th,
        max_children: 100,
        max_clusters: 4,
        extra_delimiters: opts.drain.extra_delimiters.clone(),
        ..DrainConfig::default()
    };
    let mut engine = Drain::new(cfg);
    let _ = engine.add_log_message(left);
    let (cid, is_new) = engine.add_log_message(right);
    if is_new {
        return false;
    }

    let template = engine.clusters().get(&cid).map(|c| c.template()).unwrap_or_default();
    group.signature = xxh3_64(template.as_bytes());
    group.drain3_template = Some(template);
    group.drain3_cluster_id = Some(cid);
    true
}
