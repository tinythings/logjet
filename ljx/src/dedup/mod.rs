//! Log deduplication pipeline.
//!
//! OTel-native: unpack → bucket → exact → (canon) → (drain) → emit.
//! Stages in parentheses are only active in hash2/full modes.

pub mod bucket;
pub mod canon;
pub mod canon_freetext;
pub mod canon_json;
pub mod canon_kv;
pub mod detect;
pub mod drain3;
pub mod emit;
pub mod exact;
pub mod flat_record;
pub mod unpack;

use flat_record::{BucketKey, BucketKeyKind, FlatRecord};

/// Deduplication aggressiveness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DedupMode {
    /// Stage 2 only: collapse byte-identical bodies.
    Exact,
    /// Stages 2 + 3: canonicalise then hash.
    Hash2,
    /// Stages 2 + 3 + 4: canon + Drain3 on residual singletons.
    Full,
}

/// Options for the dedup pipeline.
#[derive(Debug, Clone)]
pub struct DedupOpts {
    pub mode: DedupMode,
    pub bucket_key: BucketKeyKind,
    pub drain: DrainOpts,
}

/// Drain3-specific options (only used in Full mode).
#[derive(Debug, Clone)]
pub struct DrainOpts {
    pub sim_th: f64,
    pub depth: i64,
    pub extra_delimiters: Vec<String>,
}

impl Default for DrainOpts {
    fn default() -> Self {
        Self { sim_th: 0.7, depth: 3, extra_delimiters: Vec::new() }
    }
}

impl Default for DedupOpts {
    fn default() -> Self {
        Self { mode: DedupMode::Hash2, bucket_key: BucketKeyKind::Default, drain: DrainOpts::default() }
    }
}

/// One collapsed group of records sharing the same dedup signature.
#[derive(Debug, Clone)]
pub struct DedupGroup {
    pub bucket_key: BucketKey,
    pub signature: u64,
    pub count: u64,
    pub representative: FlatRecord,
    pub first_seen_ns: u64,
    pub last_seen_ns: u64,
    /// Up to 3 trace_id hex strings (first, middle, last).
    pub exemplar_trace_ids: Vec<String>,
    /// Up to 3 span_id hex strings (first, middle, last).
    pub exemplar_span_ids: Vec<String>,
    /// Canonical body form (set by stage 3, None in exact mode).
    pub canonical_body: Option<String>,
    /// Body shape label (set by stage 3, None in exact mode).
    pub body_shape: Option<String>,
    /// Drain3 template with `<*>` wildcards (set by stage 4, None otherwise).
    pub drain3_template: Option<String>,
    /// Drain3 cluster ID (set by stage 4, None otherwise).
    pub drain3_cluster_id: Option<i64>,
}

impl DedupGroup {
    /// Create a new group from its first (representative) record.
    pub fn new(bucket_key: BucketKey, signature: u64, rec: FlatRecord) -> Self {
        let first_seen = effective_timestamp(&rec);
        let trace_hex = hex::encode(&rec.trace_id);
        let span_hex = hex::encode(&rec.span_id);
        Self {
            bucket_key,
            signature,
            count: 1,
            first_seen_ns: first_seen,
            last_seen_ns: first_seen,
            exemplar_trace_ids: vec![trace_hex],
            exemplar_span_ids: vec![span_hex],
            canonical_body: None,
            body_shape: None,
            drain3_template: None,
            drain3_cluster_id: None,
            representative: rec,
        }
    }

    /// Absorb another record into this group.
    pub fn absorb(&mut self, rec: &FlatRecord) {
        self.count += 1;
        let ts = effective_timestamp(rec);
        if ts < self.first_seen_ns {
            self.first_seen_ns = ts;
        }
        if ts > self.last_seen_ns {
            self.last_seen_ns = ts;
        }
        collect_exemplar(&mut self.exemplar_trace_ids, &rec.trace_id);
        collect_exemplar(&mut self.exemplar_span_ids, &rec.span_id);
    }

    /// Merge another group into this one (canon stage).
    pub fn merge_group(&mut self, other: DedupGroup) {
        self.count += other.count;
        if other.first_seen_ns < self.first_seen_ns {
            self.first_seen_ns = other.first_seen_ns;
        }
        if other.last_seen_ns > self.last_seen_ns {
            self.last_seen_ns = other.last_seen_ns;
        }
    }
}

/// Effective timestamp: prefer time_unix_nano, fall back to observed.
fn effective_timestamp(rec: &FlatRecord) -> u64 {
    if rec.time_unix_nano != 0 { rec.time_unix_nano } else { rec.observed_time_unix_nano }
}

/// Collect up to 3 exemplars: first, last, and (eventually) middle.
/// During absorption we keep first and update last. The middle exemplar
/// is computed lazily if count > 2 — here we just keep overwriting
/// slot 1 (last seen so far). On final read, slots are [first, middle, last]
/// if count ≥ 3, or [first, last] if count == 2.
fn collect_exemplar(exemplars: &mut Vec<String>, id: &[u8]) {
    let hex = hex::encode(id);
    if hex.is_empty() || hex.chars().all(|c| c == '0') {
        return;
    }
    match exemplars.len() {
        0 => exemplars.push(hex),
        1 => exemplars.push(hex),
        2 => exemplars[1] = hex,
        _ => {
            // 3 slots: [first, middle, last]. Update last.
            exemplars[2] = hex;
        }
    }
}

/// Summary statistics from a dedup run.
#[derive(Debug, Clone, Copy)]
pub struct DedupStats {
    pub total_records: u64,
    pub group_count: u64,
}

impl DedupStats {
    pub fn reduction_pct(&self) -> f64 {
        if self.total_records == 0 {
            return 0.0;
        }
        (1.0 - (self.group_count as f64 / self.total_records as f64)) * 100.0
    }
}

/// Run the dedup pipeline.
pub fn dedup(
    records: Vec<FlatRecord>, passthrough: Vec<logjet::OwnedRecord>, output: &mut logjet::LogjetWriter<impl std::io::Write>, opts: &DedupOpts,
) -> crate::error::Result<DedupStats> {
    let buckets = bucket::group(records, opts.bucket_key);
    let groups = exact::dedup(buckets);

    let groups = if opts.mode >= DedupMode::Hash2 { canon::canon_dedup(groups) } else { groups };

    let groups = if opts.mode >= DedupMode::Full { dedup_residuals(groups, &opts.drain) } else { groups };

    let mode_label = match opts.mode {
        DedupMode::Exact => "exact",
        DedupMode::Hash2 => "hash2",
        DedupMode::Full => "full",
    };

    emit::write(output, &groups, &passthrough, mode_label)
}

/// Stage 4: run Drain3 on residual singletons within each bucket.
///
/// Groups with count > 1 are already successfully deduped — pass through.
/// Singletons are fed to Drain3 per-bucket. If Drain3 merges them, the
/// resulting group gets dedup.mode = "full/drain3" and carries the template.
fn dedup_residuals(groups: Vec<DedupGroup>, drain_opts: &DrainOpts) -> Vec<DedupGroup> {
    use drain3::{Drain, DrainConfig};
    use std::collections::HashMap;
    use xxhash_rust::xxh3::xxh3_64;

    // Partition groups by bucket key.
    let mut buckets: HashMap<BucketKey, Vec<DedupGroup>> = HashMap::new();
    for g in groups {
        buckets.entry(g.bucket_key.clone()).or_default().push(g);
    }

    let mut result: Vec<DedupGroup> = Vec::new();

    for (_key, bucket_groups) in buckets {
        let bucket_total: u64 = bucket_groups.iter().map(|g| g.count).sum();

        // Split into merged (count > 1) and residuals (singletons).
        let mut merged = Vec::new();
        let mut residuals = Vec::new();
        for g in bucket_groups {
            if g.count > 1 {
                merged.push(g);
            } else {
                residuals.push(g);
            }
        }

        // Skip Drain3 only when there is nothing meaningful to cluster.
        // Small buckets still benefit from the fuzzy pass in full mode.
        if residuals.len() < 2 || bucket_total == 0 {
            result.extend(merged);
            result.extend(residuals);
            continue;
        }

        // Run Drain3 on residuals — single pass.
        let cfg = DrainConfig {
            depth: drain_opts.depth,
            sim_th: drain_opts.sim_th,
            max_children: 100,
            max_clusters: residuals.len() / 2,
            extra_delimiters: drain_opts.extra_delimiters.clone(),
            ..DrainConfig::default()
        };
        let mut engine = Drain::new(cfg);
        let mut drain_groups: HashMap<i64, Vec<usize>> = HashMap::new();

        for (i, g) in residuals.iter().enumerate() {
            let body = g.canonical_body.as_deref().unwrap_or(&g.representative.body);
            let (cid, _) = engine.add_log_message(body);
            drain_groups.entry(cid).or_default().push(i);
        }

        // Build output groups from Drain3 clusters.
        for (cid, indices) in drain_groups {
            if indices.len() == 1 {
                // Still a singleton after Drain3 — pass through unchanged.
                let idx = indices[0];
                result.push(residuals[idx].clone());
                continue;
            }

            // Merge all residuals in this Drain3 cluster.
            let first_idx = indices[0];
            let mut group = residuals[first_idx].clone();
            for &idx in &indices[1..] {
                group.merge_group(residuals[idx].clone());
            }

            // Update signature and mode for drain3-merged groups.
            let template = engine.clusters().get(&cid).map(|c| c.template()).unwrap_or_default();
            group.signature = xxh3_64(template.as_bytes());
            group.drain3_template = Some(template);
            group.drain3_cluster_id = Some(cid);

            result.push(group);
        }

        result.extend(merged);
    }

    result
}

/// Tiny hex encoder (avoids pulling in the `hex` crate).
mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        let mut s = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            s.push(nibble(b >> 4));
            s.push(nibble(b & 0x0f));
        }
        s
    }

    fn nibble(n: u8) -> char {
        match n {
            0..=9 => (b'0' + n) as char,
            _ => (b'a' + n - 10) as char,
        }
    }
}

#[cfg(test)]
#[path = "dedup_utst.rs"]
mod dedup_utst;
