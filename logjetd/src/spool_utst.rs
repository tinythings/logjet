use super::BufferSpool;
use crate::config::{BufferConfig, BufferLimit};
use crate::protocol::WireRecord;
use logjet::RecordType;

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
