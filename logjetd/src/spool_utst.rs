use super::{BufferSpool, Spool};
use crate::config::{BufferConfig, BufferLimit, FileConfig, StorageConfig};
use crate::protocol::{WireRecord, read_record};
use logjet::RecordType;
use std::fs;
use std::path::PathBuf;
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
        crate::protocol::write_record(&mut bytes, &record).unwrap();
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
    let config = FileConfig { dir: dir.clone(), name: "bofh.logjet".to_string(), segment_size_bytes: 1024 * 1024 };

    {
        let mut spool = Spool::open(StorageConfig::File(config.clone())).unwrap();
        for seq in 1..=3 {
            spool.append(WireRecord { record_type: RecordType::Logs, seq, ts_unix_ns: seq, payload: vec![seq as u8] }).unwrap();
        }
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
    let config = FileConfig { dir: dir.clone(), name: "bofh.logjet".to_string(), segment_size_bytes: 1 };

    let mut spool = Spool::open(StorageConfig::File(config)).unwrap();
    for seq in 1..=4 {
        spool.append(WireRecord { record_type: RecordType::Logs, seq, ts_unix_ns: seq, payload: vec![seq as u8; 8] }).unwrap();
    }

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
    let mut spool =
        Spool::open(StorageConfig::File(FileConfig { dir: dir.clone(), name: "bofh.logjet".to_string(), segment_size_bytes: 1 })).unwrap();

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
        let mut spool =
            Spool::open(StorageConfig::File(FileConfig { dir: dir.clone(), name: "bofh.logjet".to_string(), segment_size_bytes: 1024 * 1024 }))
                .unwrap();
        spool.append(WireRecord { record_type: RecordType::Logs, seq: 1, ts_unix_ns: 1, payload: vec![1u8; 8] }).unwrap();
    }

    {
        let mut spool =
            Spool::open(StorageConfig::File(FileConfig { dir: dir.clone(), name: "bofh.logjet".to_string(), segment_size_bytes: 1024 * 1024 }))
                .unwrap();
        spool.append(WireRecord { record_type: RecordType::Logs, seq: 2, ts_unix_ns: 2, payload: vec![2u8; 8] }).unwrap();
    }

    assert!(dir.join("bofh.logjet").exists());
    assert!(!dir.join("bofh-1.logjet").exists());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn file_spool_preserves_stream_id_and_advances_sequence_seed_after_reopen() {
    let dir = unique_temp_dir("file-stream-id");
    let config = FileConfig { dir: dir.clone(), name: "bofh.logjet".to_string(), segment_size_bytes: 1024 * 1024 };

    let first_stream_id;
    {
        let mut spool = Spool::open(StorageConfig::File(config.clone())).unwrap();
        first_stream_id = spool.stream_id();
        assert_eq!(spool.next_sequence_seed().unwrap(), 1);
        spool.append(WireRecord { record_type: RecordType::Logs, seq: 1, ts_unix_ns: 1, payload: vec![1u8; 8] }).unwrap();
        spool.append(WireRecord { record_type: RecordType::Logs, seq: 2, ts_unix_ns: 2, payload: vec![2u8; 8] }).unwrap();
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
    let mut spool =
        Spool::open(StorageConfig::File(FileConfig { dir: dir.clone(), name: "bofh.logjet".to_string(), segment_size_bytes: 1 })).unwrap();

    for seq in 1..=3 {
        spool.append(WireRecord { record_type: RecordType::Logs, seq, ts_unix_ns: seq, payload: vec![seq as u8; 8] }).unwrap();
    }

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
    let mut spool =
        Spool::open(StorageConfig::File(FileConfig { dir: dir.clone(), name: "bofh.logjet".to_string(), segment_size_bytes: 1 })).unwrap();

    for seq in 1..=4 {
        spool.append(WireRecord { record_type: RecordType::Logs, seq, ts_unix_ns: seq, payload: vec![seq as u8; 8] }).unwrap();
    }

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
    let mut spool =
        Spool::open(StorageConfig::File(FileConfig { dir: dir.clone(), name: "bofh.logjet".to_string(), segment_size_bytes: 1 })).unwrap();

    for seq in 1..=3 {
        spool.append(WireRecord { record_type: RecordType::Logs, seq, ts_unix_ns: seq, payload: vec![seq as u8; 8] }).unwrap();
    }

    let removed = super::prune_named_segments(&dir, "bofh.logjet", Some(1), None, true).unwrap();
    assert_eq!(removed.len(), 3);
    assert_eq!(super::list_named_segments(&dir, "bofh.logjet").unwrap().len(), 4);

    fs::remove_dir_all(dir).unwrap();
}

fn unique_temp_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let dir = std::env::temp_dir().join(format!("logjetd-{label}-{nanos}-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}
