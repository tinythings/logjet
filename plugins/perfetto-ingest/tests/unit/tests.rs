//! Unit tests for the perfetto-ingest plugin.

use super::*;

// ── sqlite_reader tests ─────────────────────────────────────────────────────

#[test]
fn sqlite_reader_reads_slices_ordered_by_ts() {
    let db = test_db();
    let slices = db.read_slices().unwrap();
    assert_eq!(slices.len(), 3);
    assert_eq!(slices[0].id, 3); // ts=1000
    assert_eq!(slices[1].id, 1); // ts=5000
    assert_eq!(slices[2].id, 2); // ts=8000
}

#[test]
fn sqlite_reader_reads_slice_fields() {
    let db = test_db();
    let slices = db.read_slices().unwrap();
    let s = &slices[0];
    assert_eq!(s.id, 3);
    assert_eq!(s.ts, 1000);
    assert_eq!(s.dur, 500);
    assert_eq!(s.name.as_deref(), Some("early-slice"));
    assert_eq!(s.parent_id, None);
    assert_eq!(s.depth, 0);
    assert_eq!(s.track_id, 10);
    assert_eq!(s.arg_set_id, Some(1));
}

#[test]
fn sqlite_reader_reads_flows() {
    let db = test_db();
    let flows = db.read_flows().unwrap();
    assert_eq!(flows.len(), 1);
    assert_eq!(flows[0].slice_out, 1);
    assert_eq!(flows[0].slice_in, 2);
}

#[test]
fn sqlite_reader_reads_processes() {
    let db = test_db();
    let procs = db.read_processes().unwrap();
    assert_eq!(procs.len(), 1);
    assert_eq!(procs[0].upid, 100);
    assert_eq!(procs[0].name.as_deref(), Some("test-process"));
    assert_eq!(procs[0].pid, Some(1234));
}

#[test]
fn sqlite_reader_reads_threads() {
    let db = test_db();
    let threads = db.read_threads().unwrap();
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].utid, 200);
    assert_eq!(threads[0].name.as_deref(), Some("test-thread"));
    assert_eq!(threads[0].tid, Some(5678));
    assert_eq!(threads[0].upid, Some(100));
    assert!(threads[0].is_main_thread);
}

#[test]
fn sqlite_reader_reads_tracks() {
    let db = test_db();
    let tracks = db.read_tracks().unwrap();
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].id, 10);
    assert_eq!(tracks[0].name.as_deref(), Some("test-track"));
    assert_eq!(tracks[0].track_type.as_deref(), Some("thread_track"));
    assert_eq!(tracks[0].utid, Some(200));
}

#[test]
fn sqlite_reader_reads_all_args() {
    let db = test_db();
    let args = db.read_args(&[]).unwrap();
    assert_eq!(args.len(), 2);
    assert!(!args.is_empty());
}

#[test]
fn sqlite_reader_reads_args_by_arg_set_id() {
    let db = test_db();
    let args = db.read_args(&[1]).unwrap();
    assert_eq!(args.len(), 1);
    assert_eq!(args[0].key, "slice.name");
    assert_eq!(args[0].string_value.as_deref(), Some("early-slice"));
}

#[test]
fn sqlite_reader_reads_empty_args_for_unknown_id() {
    let db = test_db();
    let args = db.read_args(&[999]).unwrap();
    assert!(args.is_empty());
}

#[test]
fn sqlite_reader_reads_clock_snapshots() {
    let db = test_db();
    let snaps = db.read_clock_snapshots().unwrap();
    assert_eq!(snaps.len(), 2);
    assert_eq!(snaps[0].ts, 0);
    assert_eq!(snaps[0].clock_value, 1_700_000_000_000_000_000);
    assert_eq!(snaps[1].ts, 10000);
    assert_eq!(snaps[1].clock_value, 1_700_000_000_000_010_000);
}

// ── metrics_reader tests ────────────────────────────────────────────────────

#[test]
fn metrics_reader_parses_scalar_metric() {
    let json = r#"{"trace_stats": {"value": 42.5, "description": "test metric", "unit": "ms"}}"#;
    let path = temp_json("scalar", json);
    let metrics = metrics_reader::parse_metrics_json(&path).unwrap();
    assert_eq!(metrics.len(), 1);
    assert_eq!(metrics[0].name, "trace_stats");
    assert_eq!(metrics[0].scalar_value, Some(42.5));
    assert_eq!(metrics[0].description.as_deref(), Some("test metric"));
    assert_eq!(metrics[0].unit.as_deref(), Some("ms"));
}

#[test]
fn metrics_reader_parses_metric_with_labels() {
    let json = r#"{"cpu_usage": {"value": 85.0, "cpu": "cpu0", "mode": "user"}}"#;
    let path = temp_json("labels", json);
    let metrics = metrics_reader::parse_metrics_json(&path).unwrap();
    assert_eq!(metrics.len(), 1);
    assert_eq!(metrics[0].scalar_value, Some(85.0));
    assert!(metrics[0].labels.iter().any(|(k, v)| k == "cpu" && v == "cpu0"));
    assert!(metrics[0].labels.iter().any(|(k, v)| k == "mode" && v == "user"));
}

#[test]
fn metrics_reader_parses_multiple_metrics() {
    let json = r#"{"m1": {"value": 1.0}, "m2": {"value": 2.0}}"#;
    let path = temp_json("multi", json);
    let metrics = metrics_reader::parse_metrics_json(&path).unwrap();
    assert_eq!(metrics.len(), 2);
}

#[test]
fn metrics_reader_parses_nested_metric() {
    let json = r#"{"parent": {"value": 10.0, "child": {"value": 5.0}}}"#;
    let path = temp_json("nested", json);
    let metrics = metrics_reader::parse_metrics_json(&path).unwrap();
    assert_eq!(metrics.len(), 1);
    assert_eq!(metrics[0].children.len(), 1);
    assert_eq!(metrics[0].children[0].name, "child");
    assert_eq!(metrics[0].children[0].scalar_value, Some(5.0));
}

// ── helpers ─────────────────────────────────────────────────────────────────

fn temp_json(name: &str, content: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir();
    let path = dir.join(format!("perfetto-test-{name}-{}.json", std::process::id()));
    std::fs::write(&path, content).unwrap();
    path
}

fn test_db() -> super::sqlite_reader::PerfettoDb {
    let conn = rusqlite::Connection::open_in_memory().unwrap();

    conn.execute_batch(
        "
        CREATE TABLE slice (
            id INTEGER, ts INTEGER, dur INTEGER, name TEXT,
            parent_id INTEGER, track_id INTEGER, arg_set_id INTEGER, depth INTEGER
        );
        CREATE TABLE flow (id INTEGER, slice_out INTEGER, slice_in INTEGER);
        CREATE TABLE process (upid INTEGER, name TEXT, pid INTEGER);
        CREATE TABLE thread (utid INTEGER, name TEXT, tid INTEGER, upid INTEGER, is_main_thread INTEGER);
        CREATE TABLE track (id INTEGER, name TEXT, type TEXT, utid INTEGER, upid INTEGER);
        CREATE TABLE args (arg_set_id INTEGER, flat_key TEXT, string_value TEXT, int_value INTEGER, real_value REAL);
        CREATE TABLE clock_snapshot (ts INTEGER, clock_value INTEGER, clock_id INTEGER, clock_name TEXT);

        INSERT INTO slice VALUES
            (1, 5000, 3000, 'main-slice', NULL, 10, 2, 0),
            (2, 8000, 1000, 'child-slice', 1, 10, NULL, 1),
            (3, 1000,  500, 'early-slice', NULL, 10, 1, 0);
        INSERT INTO flow VALUES (1, 1, 2);
        INSERT INTO process VALUES (100, 'test-process', 1234);
        INSERT INTO thread VALUES (200, 'test-thread', 5678, 100, 1);
        INSERT INTO track VALUES (10, 'test-track', 'thread_track', 200, NULL);
        INSERT INTO args VALUES (1, 'slice.name', 'early-slice', NULL, NULL);
        INSERT INTO args VALUES (2, 'slice.name', 'main-slice', NULL, NULL);
        INSERT INTO clock_snapshot VALUES
            (0, 1700000000000000000, 1, 'REALTIME'),
            (10000, 1700000000000010000, 1, 'REALTIME');
        ",
    )
    .unwrap();

    super::sqlite_reader::PerfettoDb { conn }
}

// ── timestamp tests ─────────────────────────────────────────────────────────

fn make_snapshots(pairs: &[(i64, i64)]) -> Vec<crate::sqlite_reader::PerfettoClockSnapshot> {
    pairs.iter().map(|(ts, cv)| crate::sqlite_reader::PerfettoClockSnapshot { ts: *ts, clock_value: *cv }).collect()
}

#[test]
fn timestamp_to_realtime_interpolates_between_snapshots() {
    let snaps = make_snapshots(&[(0, 1_700_000_000_000_000_000), (10000, 1_700_000_000_000_010_000)]);
    let conv = timestamp::TimestampConverter::new(snaps, timestamp::TimestampPolicy::BestEffort);
    let result = conv.to_realtime(5000).unwrap();
    assert_eq!(result, Some(1_700_000_000_000_005_000));
}

#[test]
fn timestamp_to_realtime_exact_snapshot() {
    let snaps = make_snapshots(&[(0, 1_700_000_000_000_000_000), (10000, 1_700_000_000_000_010_000)]);
    let conv = timestamp::TimestampConverter::new(snaps, timestamp::TimestampPolicy::BestEffort);
    let result = conv.to_realtime(0).unwrap();
    assert_eq!(result, Some(1_700_000_000_000_000_000));
}

#[test]
fn timestamp_to_realtime_after_last_snapshot() {
    let snaps = make_snapshots(&[(0, 1_700_000_000_000_000_000), (10000, 1_700_000_000_000_010_000)]);
    let conv = timestamp::TimestampConverter::new(snaps, timestamp::TimestampPolicy::BestEffort);
    let result = conv.to_realtime(20000).unwrap();
    assert_eq!(result, Some(1_700_000_000_000_020_000));
}

#[test]
fn timestamp_to_realtime_before_first_snapshot_best_effort() {
    let snaps = make_snapshots(&[(1000, 1_700_000_000_001_000_000)]);
    let conv = timestamp::TimestampConverter::new(snaps, timestamp::TimestampPolicy::BestEffort);
    let result = conv.to_realtime(500).unwrap();
    assert_eq!(result, Some(1_700_000_000_000_999_500)); // first.clock_value - (first.ts - trace_ts)
}

#[test]
fn timestamp_to_realtime_before_first_snapshot_require_fails() {
    let snaps = make_snapshots(&[(1000, 1_700_000_000_001_000_000)]);
    let conv = timestamp::TimestampConverter::new(snaps, timestamp::TimestampPolicy::RequireRealtime);
    assert!(conv.to_realtime(500).is_err());
}

#[test]
fn timestamp_to_realtime_empty_snapshots_best_effort_returns_none() {
    let conv = timestamp::TimestampConverter::new(vec![], timestamp::TimestampPolicy::BestEffort);
    let result = conv.to_realtime(500).unwrap();
    assert_eq!(result, None);
}

#[test]
fn timestamp_to_realtime_empty_snapshots_require_fails() {
    let conv = timestamp::TimestampConverter::new(vec![], timestamp::TimestampPolicy::RequireRealtime);
    assert!(conv.to_realtime(500).is_err());
}

#[test]
fn timestamp_has_realtime() {
    let conv = timestamp::TimestampConverter::new(vec![], timestamp::TimestampPolicy::BestEffort);
    assert!(!conv.has_realtime());

    let snaps = make_snapshots(&[(0, 1_700_000_000_000_000_000)]);
    let conv = timestamp::TimestampConverter::new(snaps, timestamp::TimestampPolicy::BestEffort);
    assert!(conv.has_realtime());
}
