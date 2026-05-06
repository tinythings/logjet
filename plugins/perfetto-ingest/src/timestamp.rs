//! Trace-clock to Unix epoch timestamp conversion.
//!
//! Uses the `clock_snapshot` table from the Perfetto SQLite export to convert
//! trace-clock timestamps (typically CLOCK_MONOTONIC or CLOCK_BOOTTIME) to
//! Unix epoch nanoseconds via REALTIME clock snapshots.

#![allow(dead_code)]

use crate::sqlite_reader::PerfettoClockSnapshot;

/// Controls behaviour when realtime conversion is unavailable for a timestamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimestampPolicy {
    /// Return an error if realtime conversion is unavailable.
    RequireRealtime,
    /// Return `None` when realtime is unavailable (caller should annotate).
    BestEffort,
}

/// Converts trace-clock timestamps to Unix epoch nanoseconds using
/// REALTIME clock snapshots.
pub struct TimestampConverter {
    snapshots: Vec<PerfettoClockSnapshot>,
    policy: TimestampPolicy,
}

impl TimestampConverter {
    /// Creates a new converter from the given clock snapshots.
    ///
    /// Snapshots must be sorted by `ts` ascending.
    pub fn new(snapshots: Vec<PerfettoClockSnapshot>, policy: TimestampPolicy) -> Self {
        Self { snapshots, policy }
    }

    /// Converts a trace-clock timestamp to Unix epoch nanoseconds.
    ///
    /// Returns:
    /// - `Some(ns)` on success
    /// - `None` under BestEffort policy when realtime is unavailable
    /// - `Err` under RequireRealtime policy when realtime is unavailable
    pub fn to_realtime(&self, trace_ts: i64) -> Result<Option<u64>, String> {
        if self.snapshots.is_empty() {
            match self.policy {
                TimestampPolicy::RequireRealtime => {
                    return Err("no REALTIME clock snapshots available for timestamp conversion".to_string());
                }
                TimestampPolicy::BestEffort => return Ok(None),
            }
        }

        // Binary search for the first snapshot with ts > trace_ts.
        let idx = self.snapshots.partition_point(|s| s.ts <= trace_ts);

        if idx == 0 {
            // Before first snapshot: interpolate backwards.
            let first = &self.snapshots[0];
            match self.policy {
                TimestampPolicy::RequireRealtime => {
                    return Err(format!(
                        "trace timestamp {trace_ts} is before first REALTIME snapshot at {}",
                        first.ts
                    ));
                }
                TimestampPolicy::BestEffort => {
                    let delta = first.ts - trace_ts;
                    let realtime = (first.clock_value - delta).max(0) as u64;
                    return Ok(Some(realtime));
                }
            }
        }

        if idx >= self.snapshots.len() {
            // After last snapshot: extrapolate forwards.
            let last = &self.snapshots[self.snapshots.len() - 1];
            let delta = trace_ts - last.ts;
            Ok(Some((last.clock_value + delta).max(0) as u64))
        } else {
            // Between two snapshots: linear interpolation.
            let prev = &self.snapshots[idx - 1];
            let next = &self.snapshots[idx];

            let range_ts = next.ts - prev.ts;
            let range_realtime = next.clock_value - prev.clock_value;

            if range_ts == 0 {
                Ok(Some(prev.clock_value as u64))
            } else {
                let offset = trace_ts - prev.ts;
                let realtime = prev.clock_value
                    + (range_realtime * offset) / range_ts;
                Ok(Some(realtime.max(0) as u64))
            }
        }
    }

    /// Returns whether the converter has snapshots (i.e., realtime is available).
    pub fn has_realtime(&self) -> bool {
        !self.snapshots.is_empty()
    }
}
