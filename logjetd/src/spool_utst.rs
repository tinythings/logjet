use super::{BufferSpool, Spool};
use crate::config::{BufferConfig, BufferLimit, FileConfig, FsyncPolicy, StorageConfig};
use crate::protocol::{WireRecord, read_record};
use logjet::RecordType;
use logjet::{LogjetReader, LogjetWriter, WriterConfig};
use std::fs;
use std::io::{BufReader, Cursor};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn preserve_prefix_survives_eviction() {
    let mut spool = BufferSpool::new(BufferConfig { limit: BufferLimit::Bytes(90), keep_messages: 2 });

    for seq in 1..=5 {
        spool.append(WireRecord { record_type: RecordType::Logs, seq, ts_unix_ns: seq, payload: vec![0u8; 20] });
    }

    let kept: Vec<u64> = spool.records.iter().map(|record| record.seq).collect();
    assert_eq!(&kept[..2], &[1, 2]);
}

#[test]
fn message_limit_rotates_tail_only() {
    let mut spool = BufferSpool::new(BufferConfig { limit: BufferLimit::Messages(4), keep_messages: 2 });

    for seq in 1..=8 {
        spool.append(WireRecord { record_type: RecordType::Logs, seq, ts_unix_ns: seq, payload: vec![0u8; 8] });
    }

    let kept: Vec<u64> = spool.records.iter().map(|record| record.seq).collect();
    assert_eq!(kept, vec![1, 2, 5, 6, 7, 8]);
}

#[test]
fn replay_since_only_sends_newer_records() {
    let mut spool = Spool::open(StorageConfig::Buffer(BufferConfig { limit: BufferLimit::Messages(8), keep_messages: 1 })).unwrap();

    for seq in 1..=4 {
        spool.append(WireRecord { record_type: RecordType::Logs, seq, ts_unix_ns: seq, payload: vec![seq as u8] }).unwrap();
    }

    let mut bytes = Vec::new();
    let mut cursor = spool.replay_cursor_after(2).unwrap();
    while let Some(record) = spool.next_for_cursor(&mut cursor).unwrap() {
        crate::protocol::write_record(&mut bytes, &record, true).unwrap();
    }

    let mut reader = bytes.as_slice();
    let mut seen = Vec::new();
    while let Some(record) = read_record(&mut reader).unwrap() {
        seen.push(record.seq);
    }
    assert_eq!(seen, vec![3, 4]);
}

#[test]
fn consume_through_removes_buffer_records() {
    let mut spool = BufferSpool::new(BufferConfig { limit: BufferLimit::Messages(8), keep_messages: 2 });

    for seq in 1..=5 {
        spool.append(WireRecord { record_type: RecordType::Logs, seq, ts_unix_ns: seq, payload: vec![seq as u8] });
    }

    spool.consume_through(3);
    let kept: Vec<u64> = spool.records.iter().map(|record| record.seq).collect();
    assert_eq!(kept, vec![4, 5]);
}

#[test]
fn buffer_replay_cursor_resyncs_after_front_records_are_consumed() {
    let mut spool = Spool::open(StorageConfig::Buffer(BufferConfig { limit: BufferLimit::Messages(8), keep_messages: 1 })).unwrap();

    for seq in 1..=5 {
        spool.append(WireRecord { record_type: RecordType::Logs, seq, ts_unix_ns: seq, payload: vec![seq as u8] }).unwrap();
    }

    let mut cursor = spool.replay_cursor_after(0).unwrap();
    let first = spool.next_for_cursor(&mut cursor).unwrap().unwrap();
    assert_eq!(first.seq, 1);

    spool.consume_through(3).unwrap();

    let next = spool.next_for_cursor(&mut cursor).unwrap().unwrap();
    assert_eq!(next.seq, 4);
    let final_record = spool.next_for_cursor(&mut cursor).unwrap().unwrap();
    assert_eq!(final_record.seq, 5);
    assert!(spool.next_for_cursor(&mut cursor).unwrap().is_none());
}

#[test]
fn file_spool_consume_state_survives_reopen() {
    let dir = unique_temp_dir("file-consume");
    let config = test_file_config(&dir, 1024 * 1024);

    {
        let mut spool = Spool::open(StorageConfig::File(config.clone())).unwrap();
        for seq in 1..=3 {
            spool.append(WireRecord { record_type: RecordType::Logs, seq, ts_unix_ns: seq, payload: vec![seq as u8] }).unwrap();
        }
        spool.flush_pending().unwrap();
        spool.consume_through(2).unwrap();
    }

    {
        let spool = Spool::open(StorageConfig::File(config)).unwrap();
        let mut cursor = spool.replay_cursor_after(0).unwrap();
        let next = spool.next_for_cursor(&mut cursor).unwrap().unwrap();
        assert_eq!(next.seq, 3);
    }

    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn file_replay_cursor_skips_consumed_records_after_cleanup() {
    let dir = unique_temp_dir("file-cursor-consume");
    let config = test_file_config(&dir, 1);

    let mut spool = Spool::open(StorageConfig::File(config)).unwrap();
    for seq in 1..=4 {
        spool.append(WireRecord { record_type: RecordType::Logs, seq, ts_unix_ns: seq, payload: vec![seq as u8; 8] }).unwrap();
    }
    spool.flush_pending().unwrap();

    let mut cursor = spool.replay_cursor_after(0).unwrap();
    let first = spool.next_for_cursor(&mut cursor).unwrap().unwrap();
    assert_eq!(first.seq, 1);

    spool.consume_through(2).unwrap();

    let next = spool.next_for_cursor(&mut cursor).unwrap().unwrap();
    assert_eq!(next.seq, 3);
    let final_record = spool.next_for_cursor(&mut cursor).unwrap().unwrap();
    assert_eq!(final_record.seq, 4);
    assert!(spool.next_for_cursor(&mut cursor).unwrap().is_none());

    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn list_named_segments_orders_numeric_suffixes() {
    let dir = unique_temp_dir("segments");
    fs::write(dir.join("bofh-2.logjet"), b"x").unwrap();
    fs::write(dir.join("bofh.logjet"), b"x").unwrap();
    fs::write(dir.join("bofh-10.logjet"), b"x").unwrap();
    fs::write(dir.join("bofh-1.logjet"), b"x").unwrap();
    fs::write(dir.join("other.logjet"), b"x").unwrap();

    let segments = super::list_named_segments(&dir, "bofh.logjet").unwrap();
    let names: Vec<String> = segments.iter().map(|segment| segment.path.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names, vec!["bofh.logjet", "bofh-1.logjet", "bofh-2.logjet", "bofh-10.logjet"]);

    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn file_spool_rotates_when_segment_size_is_exceeded() {
    let dir = unique_temp_dir("file-rotate");
    let mut spool = Spool::open(StorageConfig::File(test_file_config(&dir, 1))).unwrap();

    for seq in 1..=2 {
        spool.append(WireRecord { record_type: RecordType::Logs, seq, ts_unix_ns: seq, payload: vec![0u8; 8] }).unwrap();
    }

    assert!(dir.join("bofh.logjet").exists());
    assert!(dir.join("bofh-1.logjet").exists());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn file_spool_reuses_existing_non_full_segment() {
    let dir = unique_temp_dir("file-reuse");
    {
        let mut spool = Spool::open(StorageConfig::File(test_file_config(&dir, 1024 * 1024))).unwrap();
        spool.append(WireRecord { record_type: RecordType::Logs, seq: 1, ts_unix_ns: 1, payload: vec![1u8; 8] }).unwrap();
        spool.flush_pending().unwrap();
    }

    {
        let mut spool = Spool::open(StorageConfig::File(test_file_config(&dir, 1024 * 1024))).unwrap();
        spool.append(WireRecord { record_type: RecordType::Logs, seq: 2, ts_unix_ns: 2, payload: vec![2u8; 8] }).unwrap();
        spool.flush_pending().unwrap();
    }

    assert!(dir.join("bofh.logjet").exists());
    assert!(!dir.join("bofh-1.logjet").exists());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn file_spool_preserves_stream_id_and_advances_sequence_seed_after_reopen() {
    let dir = unique_temp_dir("file-stream-id");
    let config = test_file_config(&dir, 1024 * 1024);

    let first_stream_id;
    {
        let mut spool = Spool::open(StorageConfig::File(config.clone())).unwrap();
        first_stream_id = spool.stream_id();
        assert_eq!(spool.next_sequence_seed().unwrap(), 1);
        spool.append(WireRecord { record_type: RecordType::Logs, seq: 1, ts_unix_ns: 1, payload: vec![1u8; 8] }).unwrap();
        spool.append(WireRecord { record_type: RecordType::Logs, seq: 2, ts_unix_ns: 2, payload: vec![2u8; 8] }).unwrap();
        spool.flush_pending().unwrap();
    }

    {
        let spool = Spool::open(StorageConfig::File(config)).unwrap();
        assert_eq!(spool.stream_id(), first_stream_id);
        assert_eq!(spool.next_sequence_seed().unwrap(), 3);
    }

    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn summarise_named_segments_reports_sequence_ranges() {
    let dir = unique_temp_dir("segment-summary");
    let mut spool = Spool::open(StorageConfig::File(test_file_config(&dir, 1))).unwrap();

    for seq in 1..=3 {
        spool.append(WireRecord { record_type: RecordType::Logs, seq, ts_unix_ns: seq, payload: vec![seq as u8; 8] }).unwrap();
    }
    spool.flush_pending().unwrap();

    let summaries = super::summarise_named_segments(&dir, "bofh.logjet").unwrap();
    assert!(summaries.len() >= 2);
    assert_eq!(summaries[0].first_seq, Some(1));
    assert_eq!(summaries[0].last_seq, Some(1));
    assert!(summaries.iter().any(|summary| summary.record_count == 0));
    assert!(summaries.iter().any(|summary| summary.last_seq == Some(3)));

    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn prune_named_segments_by_file_count_keeps_newest_segment() {
    let dir = unique_temp_dir("segment-prune-count");
    let mut spool = Spool::open(StorageConfig::File(test_file_config(&dir, 1))).unwrap();

    for seq in 1..=4 {
        spool.append(WireRecord { record_type: RecordType::Logs, seq, ts_unix_ns: seq, payload: vec![seq as u8; 8] }).unwrap();
    }
    spool.flush_pending().unwrap();

    let removed = super::prune_named_segments(&dir, "bofh.logjet", Some(2), None, false).unwrap();
    assert_eq!(removed.len(), 3);

    let names: Vec<String> = super::list_named_segments(&dir, "bofh.logjet")
        .unwrap()
        .into_iter()
        .map(|segment| segment.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.len(), 2);
    assert_eq!(names, vec!["bofh-3.logjet", "bofh-4.logjet"]);

    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn prune_named_segments_dry_run_does_not_remove_files() {
    let dir = unique_temp_dir("segment-prune-dry-run");
    let mut spool = Spool::open(StorageConfig::File(test_file_config(&dir, 1))).unwrap();

    for seq in 1..=3 {
        spool.append(WireRecord { record_type: RecordType::Logs, seq, ts_unix_ns: seq, payload: vec![seq as u8; 8] }).unwrap();
    }
    spool.flush_pending().unwrap();

    let removed = super::prune_named_segments(&dir, "bofh.logjet", Some(1), None, true).unwrap();
    assert_eq!(removed.len(), 3);
    assert_eq!(super::list_named_segments(&dir, "bofh.logjet").unwrap().len(), 4);

    fs::remove_dir_all(dir).unwrap();
}

fn unique_temp_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let dir = std::env::temp_dir().join(format!("ljd-{label}-{nanos}-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn test_file_config(dir: &Path, segment_size_bytes: u64) -> FileConfig {
    FileConfig {
        dir: dir.to_path_buf(),
        name: "bofh.logjet".to_string(),
        segment_size_bytes,
        fsync: FsyncPolicy::None,
        codec: logjet::Codec::Lz4,
        block_alignment: 0,
        max_total_bytes: 0,
    }
}

#[test]
fn retention_deletes_oldest_segments_when_over_limit() {
    let dir = unique_temp_dir("retention");
    let mut config = test_file_config(&dir, 1);
    config.max_total_bytes = 300;

    let mut spool = Spool::open(StorageConfig::File(config)).unwrap();
    for seq in 1..=6 {
        spool.append(WireRecord { record_type: RecordType::Logs, seq, ts_unix_ns: seq, payload: vec![seq as u8; 32] }).unwrap();
    }
    spool.flush_pending().unwrap();

    let segments: Vec<_> =
        fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("logjet")).collect();

    let total: u64 = segments.iter().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum();
    assert!(total <= 300, "total {total} should be <= 300");
    assert!(segments.len() < 6, "some segments should have been pruned");

    fs::remove_dir_all(dir).unwrap();
}

fn count_readable_records(dir: &Path) -> u64 {
    let mut total = 0u64;
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|x| x.to_str()) != Some("logjet") {
            continue;
        }
        let file = fs::File::open(&path).unwrap();
        let mut reader = logjet::LogjetReader::new(std::io::BufReader::new(file));
        while reader.next_record().unwrap().is_some() {
            total += 1;
        }
    }
    total
}

fn file_config_with_codec(dir: &Path, codec: logjet::Codec) -> FileConfig {
    FileConfig {
        dir: dir.to_path_buf(),
        name: "test.logjet".to_string(),
        segment_size_bytes: 1_000_000,
        fsync: FsyncPolicy::None,
        codec,
        block_alignment: 0,
        max_total_bytes: 0,
    }
}

/// Dropping a spool MUST flush all pending data to disk — no silent data loss.
/// This is critical for automotive/embedded: power loss can happen any time.
#[test]
fn drop_flushes_all_zstd() {
    let dir = unique_temp_dir("drop-zstd");
    let config = file_config_with_codec(&dir, logjet::Codec::Zstd);
    let n = 50u64;

    {
        let mut spool = Spool::open(StorageConfig::File(config)).unwrap();
        for seq in 1..=n {
            spool
                .append(WireRecord { record_type: RecordType::Logs, seq, ts_unix_ns: 1_700_000_000_000_000_000 + seq, payload: vec![seq as u8; 200] })
                .unwrap();
        }
        // No explicit flush. Drop must handle it.
    }

    let recovered = count_readable_records(&dir);
    assert_eq!(recovered, n, "drop must flush: recovered {recovered}/{n}");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn drop_flushes_all_lz4() {
    let dir = unique_temp_dir("drop-lz4");
    let config = file_config_with_codec(&dir, logjet::Codec::Lz4);
    let n = 50u64;

    {
        let mut spool = Spool::open(StorageConfig::File(config)).unwrap();
        for seq in 1..=n {
            spool
                .append(WireRecord { record_type: RecordType::Logs, seq, ts_unix_ns: 1_700_000_000_000_000_000 + seq, payload: vec![seq as u8; 200] })
                .unwrap();
        }
    }

    let recovered = count_readable_records(&dir);
    assert_eq!(recovered, n, "drop must flush: recovered {recovered}/{n}");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn drop_flushes_all_none() {
    let dir = unique_temp_dir("drop-none");
    let config = file_config_with_codec(&dir, logjet::Codec::None);
    let n = 50u64;

    {
        let mut spool = Spool::open(StorageConfig::File(config)).unwrap();
        for seq in 1..=n {
            spool
                .append(WireRecord { record_type: RecordType::Logs, seq, ts_unix_ns: 1_700_000_000_000_000_000 + seq, payload: vec![seq as u8; 200] })
                .unwrap();
        }
    }

    let recovered = count_readable_records(&dir);
    assert_eq!(recovered, n, "drop must flush: recovered {recovered}/{n}");
    fs::remove_dir_all(dir).unwrap();
}

/// Simulates the plugin client pattern: many rapid appends of variable sizes,
/// then drop. Must recover every single record.
#[test]
fn drop_flushes_rapid_variable_payloads() {
    let dir = unique_temp_dir("drop-variable");
    let config = file_config_with_codec(&dir, logjet::Codec::Zstd);
    let n = 200u64;

    {
        let mut spool = Spool::open(StorageConfig::File(config)).unwrap();
        for seq in 1..=n {
            let size = match seq % 10 {
                0 => 4000,
                1..=3 => 500,
                _ => 100,
            };
            spool
                .append(WireRecord {
                    record_type: RecordType::Logs,
                    seq,
                    ts_unix_ns: 1_700_000_000_000_000_000 + seq,
                    payload: vec![seq as u8; size],
                })
                .unwrap();
        }
    }

    let recovered = count_readable_records(&dir);
    assert_eq!(recovered, n, "drop must flush variable payloads: recovered {recovered}/{n}");
    fs::remove_dir_all(dir).unwrap();
}

/// Full pipeline: build_otlp_payload → FileSpool(Zstd) → read → protobuf decode.
/// Uses realistic variable-sized records.
#[test]
fn plugin_protobuf_survives_spool_round_trip() {
    use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
    use prost::Message;

    let dir = unique_temp_dir("proto-roundtrip");
    let config = file_config_with_codec(&dir, logjet::Codec::Zstd);
    let base_ts = 1_700_000_000_000_000_000u64;

    // Build payloads of various sizes — matching real ingest patterns.

    let mut expected: Vec<(u64, Vec<u8>)> = Vec::new();

    {
        let mut spool = Spool::open(StorageConfig::File(config)).unwrap();

        for seq in 1..=200u64 {
            let body_size = match seq % 10 {
                0 => 3000, // large JSON-like body

                1..=3 => 500, // medium

                _ => 50, // small
            };

            let body: String = (0..body_size).map(|i| (b'A' + (i % 26) as u8) as char).collect();

            let attrs = vec![
                ("service.name".to_string(), format!("svc-{}", seq % 5)),
                ("scope.name".to_string(), format!("scope-{}", seq % 3)),
                ("stress.msg_type".to_string(), "12".to_string()),
                ("stress.record_nr".to_string(), format!("{}", seq * 1000)),
            ];
            let payload = crate::plugin::build_otlp_payload(
                base_ts + seq,
                9, // INFO
                Some("INFO"),
                &body,
                &attrs,
            );
            // Sanity: must decode before storing.
            ExportLogsServiceRequest::decode(payload.as_slice()).unwrap_or_else(|e| panic!("seq={seq} pre-store decode failed: {e}"));

            spool.append(WireRecord { record_type: RecordType::Logs, seq, ts_unix_ns: base_ts + seq, payload: payload.clone() }).unwrap();
            expected.push((seq, payload));
        }
        // Drop flushes.
    }

    // Read back and verify every record decodes as valid protobuf.
    let mut recovered = 0usize;
    for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|x| x.to_str()) != Some("logjet") {
            continue;
        }
        let file = fs::File::open(&path).unwrap();
        let mut reader = logjet::LogjetReader::new(std::io::BufReader::new(file));
        while let Some(record) = reader.next_record().unwrap() {
            let (exp_seq, exp_payload) = &expected[recovered];
            assert_eq!(record.seq, *exp_seq, "seq mismatch at index {recovered}");
            assert_eq!(
                record.payload.len(),
                exp_payload.len(),
                "payload len mismatch at seq={}: wrote {} read {}",
                exp_seq,
                exp_payload.len(),
                record.payload.len(),
            );
            assert_eq!(record.payload, *exp_payload, "payload bytes mismatch at seq={exp_seq}");
            ExportLogsServiceRequest::decode(record.payload.as_slice()).unwrap_or_else(|e| panic!("seq={exp_seq} post-read decode failed: {e}"));
            recovered += 1;
        }
    }
    assert_eq!(recovered, expected.len(), "expected {} records, got {recovered}", expected.len());

    fs::remove_dir_all(dir).unwrap();
}

/// Proves that flush_pending before drop recovers everything, any codec.
#[test]
fn graceful_flush_recovers_all_zstd() {
    let dir = unique_temp_dir("graceful-zstd");
    let config = file_config_with_codec(&dir, logjet::Codec::Zstd);
    let records_written = 50u64;

    {
        let mut spool = Spool::open(StorageConfig::File(config)).unwrap();
        for seq in 1..=records_written {
            spool
                .append(WireRecord { record_type: RecordType::Logs, seq, ts_unix_ns: 1_700_000_000_000_000_000 + seq, payload: vec![seq as u8; 200] })
                .unwrap();
        }
        spool.flush_pending().unwrap();
    }

    let recovered = count_readable_records(&dir);
    assert_eq!(recovered, records_written, "with flush: got {recovered}/{records_written}");

    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn graceful_flush_recovers_all_lz4() {
    let dir = unique_temp_dir("graceful-lz4");
    let config = file_config_with_codec(&dir, logjet::Codec::Lz4);
    let records_written = 50u64;

    {
        let mut spool = Spool::open(StorageConfig::File(config)).unwrap();
        for seq in 1..=records_written {
            spool
                .append(WireRecord { record_type: RecordType::Logs, seq, ts_unix_ns: 1_700_000_000_000_000_000 + seq, payload: vec![seq as u8; 200] })
                .unwrap();
        }
        spool.flush_pending().unwrap();
    }

    let recovered = count_readable_records(&dir);
    assert_eq!(recovered, records_written, "with flush: got {recovered}/{records_written}");

    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn verify_otlp_payload_accepts_valid_plugin_payload() {
    let payload = crate::plugin::build_otlp_payload(
        1_700_000_000_000_000_123,
        9,
        Some("INFO"),
        "hello from plugin",
        &[("service.name".to_string(), "svc".to_string())],
    );
    let record = logjet::OwnedRecord { record_type: RecordType::Logs, seq: 1, ts_unix_ns: 1_700_000_000_000_000_123, payload };

    super::verify_otlp_payload(&record).unwrap();
}

#[test]
fn verify_otlp_payload_rejects_invalid_log_payload() {
    let record =
        logjet::OwnedRecord { record_type: RecordType::Logs, seq: 1, ts_unix_ns: 1_700_000_000_000_000_123, payload: b"not otlp protobuf".to_vec() };

    let err = super::verify_otlp_payload(&record).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn inspect_reader_reports_invalid_otlp_payloads_without_block_errors() {
    let valid_payload = crate::plugin::build_otlp_payload(
        1_700_000_000_000_000_123,
        9,
        Some("INFO"),
        "valid payload",
        &[("service.name".to_string(), "svc".to_string())],
    );

    let mut writer = LogjetWriter::with_config(Cursor::new(Vec::new()), WriterConfig { codec: logjet::Codec::Lz4, ..Default::default() });
    writer.push(RecordType::Logs, 1, 1_700_000_000_000_000_123, &valid_payload).unwrap();
    writer.push(RecordType::Logs, 2, 1_700_000_000_000_000_124, b"definitely not otlp").unwrap();
    writer.push(RecordType::Logs, 3, 1_700_000_000_000_000_125, &valid_payload).unwrap();
    let bytes = writer.into_inner().unwrap().into_inner();

    let mut reader = LogjetReader::new(BufReader::new(Cursor::new(bytes)));
    let summary = super::inspect_reader(&mut reader, true).unwrap();

    assert_eq!(summary.reader_stats.blocks_bad, 0);
    assert_eq!(summary.reader_stats.records_ok, 3);
    assert_eq!(summary.otlp_verified, 2);
    assert_eq!(summary.otlp_failed, 1);
}
