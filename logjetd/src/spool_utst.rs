use super::BufferSpool;
use crate::config::{BufferConfig, BufferLimit};
use crate::protocol::{WireRecord, read_record};
use logjet::RecordType;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn preserve_prefix_survives_eviction() {
    let mut spool = BufferSpool::new(BufferConfig {
        limit: BufferLimit::Bytes(90),
        keep_messages: 2,
    });

    for seq in 1..=5 {
        spool.append(WireRecord {
            record_type: RecordType::Logs,
            seq,
            ts_unix_ns: seq,
            payload: vec![0u8; 20],
        });
    }

    let kept: Vec<u64> = spool.records.iter().map(|record| record.seq).collect();
    assert_eq!(&kept[..2], &[1, 2]);
}

#[test]
fn message_limit_rotates_tail_only() {
    let mut spool = BufferSpool::new(BufferConfig {
        limit: BufferLimit::Messages(4),
        keep_messages: 2,
    });

    for seq in 1..=8 {
        spool.append(WireRecord {
            record_type: RecordType::Logs,
            seq,
            ts_unix_ns: seq,
            payload: vec![0u8; 8],
        });
    }

    let kept: Vec<u64> = spool.records.iter().map(|record| record.seq).collect();
    assert_eq!(kept, vec![1, 2, 5, 6, 7, 8]);
}

#[test]
fn replay_since_only_sends_newer_records() {
    let mut spool = BufferSpool::new(BufferConfig {
        limit: BufferLimit::Messages(8),
        keep_messages: 1,
    });

    for seq in 1..=4 {
        spool.append(WireRecord {
            record_type: RecordType::Logs,
            seq,
            ts_unix_ns: seq,
            payload: vec![seq as u8],
        });
    }

    let mut bytes = Vec::new();
    let mut last_seq = 2;
    let sent_any = spool.replay_since(&mut bytes, &mut last_seq).unwrap();
    assert!(sent_any);
    assert_eq!(last_seq, 4);

    let mut reader = bytes.as_slice();
    let mut seen = Vec::new();
    while let Some(record) = read_record(&mut reader).unwrap() {
        seen.push(record.seq);
    }
    assert_eq!(seen, vec![3, 4]);
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
    let names: Vec<String> = segments
        .iter()
        .map(|segment| segment.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["bofh.logjet", "bofh-1.logjet", "bofh-2.logjet", "bofh-10.logjet"]);

    fs::remove_dir_all(dir).unwrap();
}

fn unique_temp_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("logjetd-{label}-{nanos}-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}
