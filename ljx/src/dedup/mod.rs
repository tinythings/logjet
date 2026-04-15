//! Log deduplication pipeline.
//!
//! OTel-native: unpack → bucket → exact → (canon) → (drain) → emit.
//! Stages in parentheses are only active in hash2/full modes.

pub mod bucket;
#[allow(dead_code)]
pub mod canon_freetext;
#[allow(dead_code)]
pub mod canon_json;
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
}

impl Default for DedupOpts {
    fn default() -> Self {
        Self {
            mode: DedupMode::Hash2,
            bucket_key: BucketKeyKind::Default,
        }
    }
}

/// One collapsed group of records sharing the same dedup signature.
#[derive(Debug, Clone)]
pub struct DedupGroup {
    /// Used by canon stage (ticket 2) to scope merges within a bucket.
    #[allow(dead_code)]
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
}

/// Effective timestamp: prefer time_unix_nano, fall back to observed.
fn effective_timestamp(rec: &FlatRecord) -> u64 {
    if rec.time_unix_nano != 0 {
        rec.time_unix_nano
    } else {
        rec.observed_time_unix_nano
    }
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
    records: Vec<FlatRecord>,
    passthrough: Vec<logjet::OwnedRecord>,
    output: &mut logjet::LogjetWriter<impl std::io::Write>,
    opts: &DedupOpts,
) -> crate::error::Result<DedupStats> {
    let buckets = bucket::group(records, opts.bucket_key);
    let groups = exact::dedup(buckets);

    // TODO: hash2 (ticket 2) — canon stages
    // TODO: full  (ticket 3) — drain residuals

    let mode_label = match opts.mode {
        DedupMode::Exact => "exact",
        DedupMode::Hash2 => "hash2",
        DedupMode::Full => "full/canon",
    };

    emit::write(output, &groups, &passthrough, mode_label)
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
