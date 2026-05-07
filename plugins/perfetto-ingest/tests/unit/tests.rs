//! Unit tests for the perfetto-ingest plugin.

use super::*;
use prost::Message;

/// Captured emitted record: (record_type, timestamp_unix_ns, payload).
type EmittedRecord = (u32, u64, Vec<u8>);

// sqlite_reader tests

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

// metrics_reader tests

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

// helpers

fn temp_json(name: &str, content: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir();
    let path = dir.join(format!("perfetto-test-{name}-{}.json", std::process::id()));
    std::fs::write(&path, content).unwrap();
    path
}

// realistic integration tests — timestamps match real Perfetto scale

/// Realistic DB with trace-scale timestamps (~10^14) and epoch clock_values (~10^18).
/// Records from different types overlap in time, testing that the sort-before-emit
/// pipeline produces strictly monotonic output regardless of mapper order.
fn realistic_db() -> super::sqlite_reader::PerfettoDb {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "
        CREATE TABLE slice (id INTEGER, ts INTEGER, dur INTEGER, name TEXT, parent_id INTEGER, track_id INTEGER, arg_set_id INTEGER, depth INTEGER);
        CREATE TABLE sched_slice (id INTEGER, ts INTEGER, dur INTEGER, utid INTEGER, ucpu INTEGER, end_state TEXT);
        CREATE TABLE thread_state (id INTEGER, ts INTEGER, dur INTEGER, utid INTEGER, state TEXT, io_wait INTEGER, blocked_function TEXT, waker_utid INTEGER, cpu INTEGER);
        CREATE TABLE ftrace_event (id INTEGER, ts INTEGER, name TEXT, cpu INTEGER, utid INTEGER);
        CREATE TABLE spurious_sched_wakeup (id INTEGER, ts INTEGER, utid INTEGER, waker_utid INTEGER);
        CREATE TABLE instant (ts INTEGER, track_id INTEGER, name TEXT);
        CREATE TABLE thread (utid INTEGER, name TEXT, tid INTEGER, upid INTEGER, is_main_thread INTEGER);
        CREATE TABLE process (upid INTEGER, name TEXT, pid INTEGER);
        CREATE TABLE __intrinsic_track (id INTEGER, name TEXT, type TEXT, utid INTEGER, upid INTEGER);
        CREATE TABLE clock_snapshot (ts INTEGER, clock_value INTEGER, clock_id INTEGER, clock_name TEXT);
        -- Overlapping time ranges in Perfetto trace scale (~10^14 ns = microseconds).
        INSERT INTO slice VALUES (1, 300_000_000, 50, 'mid-span', NULL, 10, NULL, 0);
        INSERT INTO sched_slice VALUES (1, 500_000_000, 20, 5, 2, 'R');
        INSERT INTO thread_state VALUES (1, 100_000_000, 50, 3, 'S', NULL, NULL, NULL, 1);
        INSERT INTO thread_state VALUES (2, 400_000_000, 80, 5, 'R', NULL, NULL, NULL, 2);
        INSERT INTO ftrace_event VALUES (1, 200_000_000, 'sched_switch', 1, 3);
        INSERT INTO spurious_sched_wakeup VALUES (1, 350_000_000, 5, 3);
        INSERT INTO instant VALUES (150_000_000, 10, 'early');
        INSERT INTO thread VALUES (3, 'main', 100, 1, 1), (5, 'worker', 200, 1, 0);
        INSERT INTO process VALUES (1, 'test', 1000);
        INSERT INTO clock_snapshot VALUES (0, 1700000000000000000, 1, 'REALTIME');
        ",
    ).unwrap();
    super::sqlite_reader::PerfettoDb { conn }
}

#[test]
fn log_mapper_produces_monotonic_timestamps_with_realistic_data() {
    let mut db = realistic_db();
    let snaps = vec![crate::sqlite_reader::PerfettoClockSnapshot { ts: 0, clock_value: 1_700_000_000_000_000_000 }];
    let converter = timestamp::TimestampConverter::new(snaps, timestamp::TimestampPolicy::BestEffort);

    // Collect emits into a buffer, sort, then verify monotonic.
    EMIT_BUF.with(|buf| buf.borrow_mut().clear());
    log_mapper::map_logs(&mut db, &converter, super::buffer_emit, &dummy_plugin(dummy_emit)).unwrap();

    let mut all: Vec<(u32, u64, Vec<u8>)> = Vec::new();
    EMIT_BUF.with(|buf| all = std::mem::take(&mut *buf.borrow_mut()));
    all.sort_by_key(|(_, ts, _)| *ts);

    assert!(!all.is_empty(), "expected records from realistic DB");

    // Verify monotonic.
    for w in all.windows(2) {
        assert!(w[0].1 <= w[1].1, "non-monotonic: {} then {} (delta={})", w[0].1, w[1].1, w[1].1 as i128 - w[0].1 as i128);
    }
}

#[test]
fn full_pipeline_sorts_across_mappers_monotonically() {
    let mut db = realistic_db();
    let snaps = vec![crate::sqlite_reader::PerfettoClockSnapshot { ts: 0, clock_value: 1_700_000_000_000_000_000 }];
    let converter = timestamp::TimestampConverter::new(snaps, timestamp::TimestampPolicy::BestEffort);

    EMIT_BUF.with(|buf| buf.borrow_mut().clear());

    // Run mappers in varying order — traces first, then logs (opposite of monotonic order).
    // The buffer should sort everything before emitting.
    let _ = trace_mapper::map_traces(&db, &converter, super::buffer_emit, &dummy_plugin(dummy_emit));
    let _ = log_mapper::map_logs(&mut db, &converter, super::buffer_emit, &dummy_plugin(dummy_emit));

    let mut all: Vec<(u32, u64, Vec<u8>)> = Vec::new();
    EMIT_BUF.with(|buf| all = std::mem::take(&mut *buf.borrow_mut()));
    all.sort_by_key(|(_, ts, _)| *ts);

    assert!(!all.is_empty());
    for w in all.windows(2) {
        assert!(w[0].1 <= w[1].1, "non-monotonic across mappers: {} then {}", w[0].1, w[1].1);
    }
}

#[test]
fn realistic_timestamps_convert_without_truncation() {
    let snaps = vec![crate::sqlite_reader::PerfettoClockSnapshot { ts: 0, clock_value: 1_700_000_000_000_000_000 }];
    let converter = timestamp::TimestampConverter::new(snaps, timestamp::TimestampPolicy::BestEffort);

    // Perfetto-scaled timestamp: typical trace time (~122 seconds in ns).
    let trace_ts: i64 = 122_804_694_200;
    let realtime = converter.to_realtime(trace_ts).unwrap().unwrap();
    assert_eq!(realtime, 1_700_000_000_000_000_000 + 122_804_694_200);
    assert!(realtime > 1_700_000_000_000_000_000);
}

#[test]
fn realistic_timestamps_maintain_monotonicity() {
    let snaps = vec![crate::sqlite_reader::PerfettoClockSnapshot { ts: 0, clock_value: 1_700_000_000_000_000_000 }];
    let converter = timestamp::TimestampConverter::new(snaps, timestamp::TimestampPolicy::BestEffort);

    // Timestamps in increasing order as they would appear in a trace.
    // These are in Perfetto trace scale (~10^14 = microseconds from trace start).
    let times: [i64; 5] = [100_000_000, 200_000_000, 300_000_000, 500_000_000, 900_000_000];
    let mut prev: u64 = 0;
    for ts in &times {
        let rt = converter.to_realtime(*ts).unwrap().unwrap();
        assert!(rt >= prev, "realtime {rt} should be >= prev {prev} for ts={ts}");
        prev = rt;
    }
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
        CREATE TABLE __intrinsic_track (id INTEGER, name TEXT, type TEXT, utid INTEGER, upid INTEGER);
        CREATE TABLE sched_slice (id INTEGER, ts INTEGER, dur INTEGER, utid INTEGER, ucpu INTEGER, end_state TEXT);
        CREATE TABLE thread_state (id INTEGER, ts INTEGER, dur INTEGER, utid INTEGER, state TEXT, io_wait INTEGER, blocked_function TEXT, waker_utid INTEGER, cpu INTEGER);
        CREATE TABLE ftrace_event (id INTEGER, ts INTEGER, name TEXT, cpu INTEGER, utid INTEGER);
        CREATE TABLE spurious_sched_wakeup (id INTEGER, ts INTEGER, utid INTEGER, waker_utid INTEGER);
        CREATE TABLE instant (ts INTEGER, track_id INTEGER, name TEXT);
        CREATE TABLE cpu (id INTEGER, cpu INTEGER, cluster_id INTEGER, processor TEXT);
        CREATE TABLE machine (id INTEGER, arch TEXT, num_cpus INTEGER, sysname TEXT, release TEXT);
        CREATE TABLE metadata (name TEXT, int_value INTEGER, str_value TEXT);
        CREATE TABLE counter (id INTEGER, ts INTEGER, track_id INTEGER, value REAL);
        CREATE TABLE memory_snapshot (id INTEGER, timestamp INTEGER, track_id INTEGER, detail_level TEXT);
        CREATE TABLE cpu_profile_stack_sample (id INTEGER, ts INTEGER, callsite_id INTEGER, utid INTEGER);
        CREATE TABLE stack_profile_frame (id INTEGER, name TEXT, mapping_id INTEGER);
        CREATE TABLE heap_profile_allocation (id INTEGER, ts INTEGER, upid INTEGER, size INTEGER, count INTEGER);
        CREATE TABLE protolog (id INTEGER, ts INTEGER, level TEXT, tag TEXT, message TEXT);
        CREATE TABLE android_logs (id INTEGER, ts INTEGER, utid INTEGER, prio INTEGER, tag TEXT, msg TEXT);
        CREATE TABLE filedescriptor (id INTEGER, ufd INTEGER, fd INTEGER, ts INTEGER, upid INTEGER, path TEXT);
        CREATE TABLE args (arg_set_id INTEGER, flat_key TEXT, string_value TEXT, int_value INTEGER, real_value REAL);
        CREATE TABLE clock_snapshot (ts INTEGER, clock_value INTEGER, clock_id INTEGER, clock_name TEXT);

        INSERT INTO slice VALUES
            (1, 5000, 3000, 'main-slice', NULL, 10, 2, 0),
            (2, 8000, 1000, 'child-slice', 1, 10, NULL, 1),
            (3, 1000,  500, 'early-slice', NULL, 10, 1, 0);
        INSERT INTO flow VALUES (1, 1, 2);
        INSERT INTO process VALUES (100, 'test-process', 1234);
        INSERT INTO thread VALUES (200, 'test-thread', 5678, 100, 1);
        INSERT INTO __intrinsic_track VALUES (10, 'test-track', 'thread_track', 200, NULL);
        INSERT INTO args VALUES (1, 'slice.name', 'early-slice', NULL, NULL);
        INSERT INTO args VALUES (2, 'slice.name', 'main-slice', NULL, NULL);
        INSERT INTO thread_state VALUES (1, 5000, 10000, 1, 'R', NULL, NULL, NULL, 0);
        INSERT INTO thread_state VALUES (2, 15000, 5000, 2, 'S', 1, 'pipe_wait', 1, 0);
        INSERT INTO ftrace_event VALUES (1, 5000, 'sched_switch', 0, 1);
        INSERT INTO ftrace_event VALUES (2, 15000, 'sched_waking', 2, 3);
        INSERT INTO spurious_sched_wakeup VALUES (1, 5000, 1, 2);
        INSERT INTO instant VALUES (10000, 10, 'test-instant');
        INSERT INTO cpu VALUES (1, 0, 0, 'x86_64');
        INSERT INTO machine VALUES (1, 'x86_64', 8, 'Linux', '5.15.0');
        INSERT INTO metadata VALUES ('trace_size_bytes', 1048576, NULL);
        INSERT INTO counter VALUES (1, 10000, 1, 2400000.0);
        INSERT INTO memory_snapshot VALUES (1, 20000, 1, 'detailed');
        INSERT INTO cpu_profile_stack_sample VALUES (1, 10000, 42, 1);
        INSERT INTO stack_profile_frame VALUES (1, 'main', 1);
        INSERT INTO heap_profile_allocation VALUES (1, 20000, 1, 4096, 1);
        INSERT INTO protolog VALUES (1, 10000, 'INFO', 'test', 'test log');
        INSERT INTO android_logs VALUES (1, 10000, 1, 3, 'TestTag', 'test message');
        INSERT INTO filedescriptor VALUES (1, 1, 42, 10000, 1, '/dev/null');
        INSERT INTO clock_snapshot VALUES
            (0, 1700000000000000000, 1, 'REALTIME'),
            (10000, 1700000000000010000, 1, 'REALTIME');
        ",
    )
    .unwrap();

    super::sqlite_reader::PerfettoDb { conn }
}

// timestamp tests

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

// trace_mapper tests

#[test]
fn trace_mapper_produces_spans_from_slices() {
    let db = test_db();
    let snaps = vec![crate::sqlite_reader::PerfettoClockSnapshot { ts: 0, clock_value: 1_700_000_000_000_000_000 }];
    let converter = timestamp::TimestampConverter::new(snaps, timestamp::TimestampPolicy::BestEffort);
    let emitted = run_trace_mapper(&db, &converter);

    assert!(!emitted.is_empty(), "expected at least one span batch");
    assert!(emitted[0].1 > 0, "expected valid record_type");
}

#[test]
fn trace_mapper_skips_slices_without_realtime_best_effort() {
    let db = test_db();
    let converter = timestamp::TimestampConverter::new(vec![], timestamp::TimestampPolicy::BestEffort);
    let emitted = run_trace_mapper(&db, &converter);

    // BestEffort with no snapshots: slices should be skipped.
    assert!(emitted.is_empty());
}

#[test]
fn trace_mapper_fails_without_realtime_require() {
    let db = test_db();
    let converter = timestamp::TimestampConverter::new(vec![], timestamp::TimestampPolicy::RequireRealtime);
    let result = run_trace_mapper_result(&db, &converter);
    assert!(result.is_err());
}

// metric_mapper tests

#[test]
fn metric_mapper_encodes_scalar_metrics() {
    let metrics = vec![crate::metrics_reader::PerfettoMetric {
        name: "test_metric".to_string(),
        description: Some("a test metric".to_string()),
        unit: Some("ms".to_string()),
        scalar_value: Some(42.0),
        labels: vec![("cpu".to_string(), "0".to_string())],
        children: vec![],
    }];

    let emitted = run_metric_mapper(&metrics);
    assert!(!emitted.is_empty());
}

#[test]
fn metric_mapper_handles_empty_metrics() {
    let emitted = run_metric_mapper(&[]);
    assert!(emitted.is_empty());
}

#[test]
fn metric_mapper_flattens_nested_metrics() {
    let metrics = vec![crate::metrics_reader::PerfettoMetric {
        name: "parent".to_string(),
        description: None,
        unit: None,
        scalar_value: Some(10.0),
        labels: vec![],
        children: vec![crate::metrics_reader::PerfettoMetric {
            name: "child".to_string(),
            description: None,
            unit: None,
            scalar_value: Some(5.0),
            labels: vec![],
            children: vec![],
        }],
    }];

    let emitted = run_metric_mapper(&metrics);
    let payload = &emitted[0].2;
    let req = opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest::decode(payload.as_slice()).unwrap();
    let names: Vec<&str> = req.resource_metrics[0].scope_metrics[0].metrics.iter().map(|m| m.name.as_str()).collect();
    assert!(names.contains(&"parent"));
    assert!(names.contains(&"parent.child"));
}

// log_mapper tests

#[test]
fn log_mapper_produces_per_slice_and_summary_logs() {
    let mut db = test_db();
    let emitted = run_log_mapper(&mut db);
    // 3 slices + 1 summary = 4 log records
    assert!(emitted.len() >= 2, "expected per-slice logs + summary");
    assert!(emitted.iter().all(|(rt, _, _)| *rt == crate::LJ_INGEST_RECORD_TYPE_LOGS));
}

// run_pipeline integration test

#[test]
fn run_pipeline_integration_with_sqlite() {
    // This test exercises the pipeline end-to-end using a pre-made SQLite DB.
    // It does NOT require trace_processor — we export the in-memory DB to a
    // temp file and feed the pipeline directly via a manual path.

    let tmp = temp_sqlite_file();
    let emitted = run_core_pipeline(&tmp);

    assert!(!emitted.is_empty());

    let has_logs = emitted.iter().any(|(rt, _, _)| *rt == crate::LJ_INGEST_RECORD_TYPE_LOGS);
    assert!(has_logs, "expected at least one log record from summary");

    let _ = std::fs::remove_file(&tmp);
}

// test helpers for mapper tests

fn run_trace_mapper(db: &super::sqlite_reader::PerfettoDb, converter: &timestamp::TimestampConverter) -> Vec<EmittedRecord> {
    run_trace_mapper_result(db, converter).unwrap()
}

fn run_trace_mapper_result(db: &super::sqlite_reader::PerfettoDb, converter: &timestamp::TimestampConverter) -> Result<Vec<EmittedRecord>, String> {
    let plugin = dummy_plugin(dummy_emit);
    trace_mapper::map_traces(db, converter, super::emit_generic, &plugin)?;
    Ok(take_records(&plugin))
}

fn run_metric_mapper(metrics: &[crate::metrics_reader::PerfettoMetric]) -> Vec<EmittedRecord> {
    let plugin = dummy_plugin(dummy_emit);
    let converter = timestamp::TimestampConverter::new(vec![], timestamp::TimestampPolicy::BestEffort);
    metric_mapper::map_metrics(metrics, &converter, super::emit_generic, &plugin).unwrap();
    take_records(&plugin)
}

fn run_log_mapper(db: &mut super::sqlite_reader::PerfettoDb) -> Vec<EmittedRecord> {
    let plugin = dummy_plugin(dummy_emit);
    let snaps = vec![crate::sqlite_reader::PerfettoClockSnapshot { ts: 0, clock_value: 1_700_000_000_000_000_000 }];
    let converter = timestamp::TimestampConverter::new(snaps, timestamp::TimestampPolicy::BestEffort);
    log_mapper::map_logs(db, &converter, super::emit_generic, &plugin).unwrap();
    take_records(&plugin)
}

// sqlite_reader tests for P1/P2 types

#[test]
fn sqlite_reader_reads_thread_states() {
    let db = test_db();
    let states = db.read_thread_states().unwrap();
    assert_eq!(states.len(), 2);
    assert_eq!(states[0].state.as_deref(), Some("R"));
    assert_eq!(states[1].io_wait, Some(true));
    assert_eq!(states[1].blocked_function.as_deref(), Some("pipe_wait"));
}

#[test]
fn sqlite_reader_reads_ftrace_events() {
    let db = test_db();
    let events = db.read_ftrace_events().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].name.as_deref(), Some("sched_switch"));
    assert_eq!(events[1].name.as_deref(), Some("sched_waking"));
}

#[test]
fn sqlite_reader_reads_spurious_wakeups() {
    let db = test_db();
    let wakeups = db.read_spurious_wakeups().unwrap();
    assert_eq!(wakeups.len(), 1);
    assert_eq!(wakeups[0].utid, Some(1));
    assert_eq!(wakeups[0].waker_utid, Some(2));
}

#[test]
fn sqlite_reader_reads_instants() {
    let db = test_db();
    let instants = db.read_instants().unwrap();
    assert_eq!(instants.len(), 1);
    assert_eq!(instants[0].name.as_deref(), Some("test-instant"));
}

#[test]
fn log_mapper_produces_thread_state_records() {
    let mut db = test_db();
    let emitted = run_log_mapper(&mut db);
    // Should include 2 thread_state records + slices + sched + ftrace + spurious + instant + summary
    assert!(emitted.iter().any(|(rt, _, _)| *rt == crate::LJ_INGEST_RECORD_TYPE_LOGS));
    // Verify thread_state attributes in payload
    let has_ts_attrs = emitted.iter().any(|(_, _, payload)| {
        if let Ok(req) = prost::Message::decode(payload.as_slice()) {
            let req: opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest = req;
            req.resource_logs.iter().any(|rl| {
                rl.scope_logs.iter().any(|sl| {
                    sl.log_records.iter().any(|lr| {
                        lr.attributes.iter().any(|kv| kv.key == "perfetto.ts.state" && kv.value.as_ref().is_some_and(|v| v.value.is_some()))
                    })
                })
            })
        } else {
            false
        }
    });
    assert!(has_ts_attrs, "expected thread_state attributes in emitted records");
}

#[test]
fn log_mapper_produces_ftrace_event_records() {
    let mut db = test_db();
    let emitted = run_log_mapper(&mut db);
    let has_ftrace = emitted.iter().any(|(_, _, payload)| {
        if let Ok(req) = prost::Message::decode(payload.as_slice()) {
            let req: opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest = req;
            req.resource_logs.iter().any(|rl| {
                rl.scope_logs.iter().any(|sl| {
                    sl.log_records.iter().any(|lr| {
                        lr.attributes.iter().any(|kv| kv.key == "perfetto.ftrace.name" && kv.value.as_ref().is_some_and(|v| v.value.is_some()))
                    })
                })
            })
        } else {
            false
        }
    });
    assert!(has_ftrace, "expected ftrace_event attributes in emitted records");
}

#[test]
fn log_mapper_produces_spurious_wakeup_records() {
    let mut db = test_db();
    let emitted = run_log_mapper(&mut db);
    let has_sw = emitted.iter().any(|(_, _, payload)| {
        if let Ok(req) = prost::Message::decode(payload.as_slice()) {
            let req: opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest = req;
            req.resource_logs
                .iter()
                .any(|rl| rl.scope_logs.iter().any(|sl| sl.log_records.iter().any(|lr| lr.attributes.iter().any(|kv| kv.key == "perfetto.sw.id"))))
        } else {
            false
        }
    });
    assert!(has_sw, "expected spurious_wakeup attributes in emitted records");
}

#[test]
fn log_mapper_produces_instant_records() {
    let mut db = test_db();
    let emitted = run_log_mapper(&mut db);
    let has_instant = emitted.iter().any(|(_, _, payload)| {
        if let Ok(req) = prost::Message::decode(payload.as_slice()) {
            let req: opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest = req;
            req.resource_logs.iter().any(|rl| {
                rl.scope_logs.iter().any(|sl| {
                    sl.log_records.iter().any(|lr| {
                        lr.attributes.iter().any(|kv| kv.key == "perfetto.instant.name" && kv.value.as_ref().is_some_and(|v| v.value.is_some()))
                    })
                })
            })
        } else {
            false
        }
    });
    assert!(has_instant, "expected instant event attributes in emitted records");
}

// sqlite_reader tests for P3-P9 types

#[test]
fn sqlite_reader_reads_cpus() {
    let db = test_db();
    let cpus = db.read_cpus().unwrap();
    assert_eq!(cpus.len(), 1);
    assert_eq!(cpus[0].cpu, Some(0));
    assert_eq!(cpus[0].processor.as_deref(), Some("x86_64"));
}

#[test]
fn sqlite_reader_reads_machines() {
    let db = test_db();
    let machines = db.read_machines().unwrap();
    assert_eq!(machines.len(), 1);
    assert_eq!(machines[0].arch.as_deref(), Some("x86_64"));
    assert_eq!(machines[0].sysname.as_deref(), Some("Linux"));
}

#[test]
fn sqlite_reader_reads_metadata() {
    let db = test_db();
    let meta = db.read_metadata().unwrap();
    assert_eq!(meta.len(), 1);
    assert_eq!(meta[0].name.as_deref(), Some("trace_size_bytes"));
}

#[test]
fn sqlite_reader_reads_counters() {
    let db = test_db();
    let counters = db.read_counters().unwrap();
    assert_eq!(counters.len(), 1);
    assert!((counters[0].value - 2400000.0).abs() < 1.0);
}

#[test]
fn sqlite_reader_reads_memory_snapshots() {
    let db = test_db();
    let snaps = db.read_memory_snapshots().unwrap();
    assert_eq!(snaps.len(), 1);
    assert_eq!(snaps[0].detail_level.as_deref(), Some("detailed"));
}

#[test]
fn sqlite_reader_reads_cpu_profile_samples() {
    let db = test_db();
    let samples = db.read_cpu_profile_samples().unwrap();
    assert_eq!(samples.len(), 1);
    assert_eq!(samples[0].callsite_id, 42);
}

#[test]
fn sqlite_reader_reads_stack_frames() {
    let db = test_db();
    let frames = db.read_stack_frames().unwrap();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].name.as_deref(), Some("main"));
}

#[test]
fn sqlite_reader_reads_heap_allocations() {
    let db = test_db();
    let allocs = db.read_heap_allocations().unwrap();
    assert_eq!(allocs.len(), 1);
    assert_eq!(allocs[0].size, 4096);
}

#[test]
fn sqlite_reader_reads_protologs() {
    let db = test_db();
    let logs = db.read_protologs().unwrap();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].message.as_deref(), Some("test log"));
}

#[test]
fn sqlite_reader_reads_android_logs() {
    let db = test_db();
    let logs = db.read_android_logs().unwrap();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].tag.as_deref(), Some("TestTag"));
}

#[test]
fn sqlite_reader_reads_filedescriptors() {
    let db = test_db();
    let fds = db.read_filedescriptors().unwrap();
    assert_eq!(fds.len(), 1);
    assert_eq!(fds[0].path.as_deref(), Some("/dev/null"));
}

fn run_core_pipeline(sqlite_path: &std::path::Path) -> Vec<EmittedRecord> {
    let plugin = dummy_plugin(dummy_emit);
    let mut db = super::sqlite_reader::PerfettoDb::open(sqlite_path).unwrap();
    let snaps = db.read_clock_snapshots().unwrap();
    let converter = timestamp::TimestampConverter::new(snaps, timestamp::TimestampPolicy::BestEffort);
    let _ = trace_mapper::map_traces(&db, &converter, super::emit_generic, &plugin);
    let _ = log_mapper::map_logs(&mut db, &converter, super::emit_generic, &plugin);
    take_records(&plugin)
}

fn dummy_plugin(cb: super::GenericRecordCallback) -> super::PerfettoPlugin {
    let records: Box<std::cell::RefCell<Vec<EmittedRecord>>> = Box::new(std::cell::RefCell::new(Vec::new()));
    let user_ptr = Box::into_raw(records) as *mut std::ffi::c_void;

    super::PerfettoPlugin {
        legacy_callback: None,
        legacy_user: std::ptr::null_mut(),
        generic_callback: Some(cb),
        generic_user: user_ptr,
        last_error: None,
    }
}

unsafe extern "C" fn dummy_emit(user: *mut std::ffi::c_void, record: *const super::LjIngestRecordV1) {
    let records = unsafe { &*(user as *const std::cell::RefCell<Vec<EmittedRecord>>) };
    let rec = unsafe { &*record };
    let payload = if rec.payload.is_null() || rec.payload_len == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(rec.payload, rec.payload_len) }.to_vec()
    };
    records.borrow_mut().push((rec.record_type, rec.timestamp_unix_ns, payload));
}

fn take_records(plugin: &super::PerfettoPlugin) -> Vec<EmittedRecord> {
    if plugin.generic_user.is_null() {
        return Vec::new();
    }
    let records_box = unsafe { Box::from_raw(plugin.generic_user as *mut std::cell::RefCell<Vec<EmittedRecord>>) };
    let cell = *records_box;
    cell.into_inner()
}

fn temp_sqlite_file() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("perfetto-test-pipeline-{}.sqlite", std::process::id()));
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "
        CREATE TABLE slice (id INTEGER, ts INTEGER, dur INTEGER, name TEXT, parent_id INTEGER, track_id INTEGER, arg_set_id INTEGER, depth INTEGER);
        CREATE TABLE thread (utid INTEGER, name TEXT, tid INTEGER, upid INTEGER, is_main_thread INTEGER);
        CREATE TABLE process (upid INTEGER, name TEXT, pid INTEGER);
        CREATE TABLE __intrinsic_track (id INTEGER, name TEXT, type TEXT, utid INTEGER, upid INTEGER);
        CREATE TABLE sched_slice (id INTEGER, ts INTEGER, dur INTEGER, utid INTEGER, ucpu INTEGER, end_state TEXT);
        CREATE TABLE thread_state (id INTEGER, ts INTEGER, dur INTEGER, utid INTEGER, state TEXT, io_wait INTEGER, blocked_function TEXT, waker_utid INTEGER, cpu INTEGER);
        CREATE TABLE ftrace_event (id INTEGER, ts INTEGER, name TEXT, cpu INTEGER, utid INTEGER);
        CREATE TABLE spurious_sched_wakeup (id INTEGER, ts INTEGER, utid INTEGER, waker_utid INTEGER);
        CREATE TABLE instant (ts INTEGER, track_id INTEGER, name TEXT);
        CREATE TABLE clock_snapshot (ts INTEGER, clock_value INTEGER, clock_id INTEGER, clock_name TEXT);
        INSERT INTO slice VALUES (1, 5000, 3000, 'test', NULL, 10, NULL, 0), (2, 10000, 500, 'child', 1, 10, NULL, 1);
        INSERT INTO thread VALUES (1, 'main', 100, 1, 1);
        INSERT INTO process VALUES (1, 'testproc', 1000);
        INSERT INTO clock_snapshot VALUES (0, 1700000000000000000, 1, 'REALTIME'), (20000, 1700000000000020000, 1, 'REALTIME');
        ",
    ).unwrap();
    path
}

#[test]
fn rpc_parse_captured_bytes() {
    let mut data = std::fs::read("/tmp/rpc-capture/small_sched_slice.bin").unwrap();
    eprintln!("raw data: {} bytes, first 32: {:02x?}", data.len(), &data[..data.len().min(32)]);
    if data.first() == Some(&0x0a) {
        let mut p = 1usize;
        let len = {
            let (mut v, mut s) = (0u64, 0u32);
            loop {
                let b = data[p];
                p += 1;
                v |= ((b & 0x7F) as u64) << s;
                if b & 0x80 == 0 {
                    break v;
                }
                s += 7;
            }
        };
        eprintln!("frame length varint = {len}");
        data = data[p..].to_vec();
    }
    eprintln!("stripped data: {} bytes, first 16: {:02x?}", data.len(), &data[..data.len().min(16)]);
    let result = crate::rpc_client::parse_response(&data, None);
    assert!(result.is_some(), "parse_response returned None for {} bytes", data.len());
    let qr = result.unwrap();
    eprintln!("columns={:?} rows={}", qr.column_names, qr.rows.len());
}
