use std::io::Cursor;

use logjet::codec::Codec;
use logjet::format::{BLOCK_HEADER_FIXED_LEN, DEFAULT_SYNC_MARKER};
use logjet::{LogjetReader, LogjetWriter, ReaderConfig, RecordType, WriterConfig};

type TestRecord = (RecordType, u64, u64, Vec<u8>);
type ReadAllOutput = (Vec<TestRecord>, logjet::ReaderStats);

fn sample_record(index: u64) -> TestRecord {
    let record_type = match index % 3 {
        0 => RecordType::Logs,
        1 => RecordType::Metrics,
        _ => RecordType::Traces,
    };
    let seq = 1000 + index;
    let ts = 5_000_000_000 + index * 100;
    let payload = format!("otlp-batch-{index}").into_bytes();
    (record_type, seq, ts, payload)
}

fn write_records(config: WriterConfig, count: u64) -> Vec<u8> {
    let mut writer = LogjetWriter::with_config(Cursor::new(Vec::new()), config);
    for index in 0..count {
        let (record_type, seq, ts, payload) = sample_record(index);
        writer.push(record_type, seq, ts, &payload).unwrap();
    }
    writer.into_inner().unwrap().into_inner()
}

fn read_all(bytes: Vec<u8>, config: ReaderConfig) -> ReadAllOutput {
    let mut reader = LogjetReader::with_config(Cursor::new(bytes), config);
    let mut out = Vec::new();
    while let Some(record) = reader.next_record().unwrap() {
        out.push((record.record_type, record.seq, record.ts_unix_ns, record.payload));
    }
    (out, reader.stats())
}

#[test]
fn write_one_block_read_back() {
    let bytes = write_records(WriterConfig { block_target_size: 1024, codec: Codec::Lz4, ..Default::default() }, 3);

    let (records, stats) = read_all(bytes, ReaderConfig::default());
    assert_eq!(records.len(), 3);
    assert_eq!(stats.blocks_ok, 1);
    assert_eq!(stats.blocks_bad, 0);
    assert_eq!(stats.records_ok, 3);
    assert_eq!(records[0].1, 1000);
    assert_eq!(records[1].2, 5_000_000_100);
}

#[test]
fn write_many_blocks_read_all() {
    let bytes = write_records(WriterConfig { block_target_size: 48, codec: Codec::None, ..Default::default() }, 20);

    let (records, stats) = read_all(bytes, ReaderConfig::default());
    assert_eq!(records.len(), 20);
    assert!(stats.blocks_ok >= 10);
    assert_eq!(stats.blocks_bad, 0);
    assert_eq!(stats.records_ok, 20);
}

#[test]
fn corrupt_middle_block_and_recover() {
    let mut bytes = write_records(WriterConfig { block_target_size: 48, codec: Codec::None, ..Default::default() }, 12);

    let sync_positions: Vec<usize> =
        bytes.windows(DEFAULT_SYNC_MARKER.len()).enumerate().filter_map(|(idx, window)| (window == DEFAULT_SYNC_MARKER).then_some(idx)).collect();
    assert!(sync_positions.len() >= 3);

    let second = sync_positions[1];
    let corrupt_at = second + DEFAULT_SYNC_MARKER.len() + BLOCK_HEADER_FIXED_LEN + 5;
    for byte in &mut bytes[corrupt_at..corrupt_at + 6] {
        *byte ^= 0x5a;
    }

    let (records, stats) = read_all(bytes, ReaderConfig::default());
    assert!(records.len() < 12);
    assert!(records.len() >= 8);
    assert!(stats.blocks_ok >= 2);
    assert!(stats.blocks_bad >= 1);
}

#[test]
fn unknown_codec_and_version_are_rejected() {
    let mut bytes = write_records(WriterConfig::default(), 1);

    bytes[DEFAULT_SYNC_MARKER.len()] = 9;
    let (_, stats_version) = read_all(bytes.clone(), ReaderConfig::default());
    assert_eq!(stats_version.blocks_ok, 0);
    assert_eq!(stats_version.blocks_bad, 1);

    let mut bytes = write_records(WriterConfig::default(), 1);
    bytes[DEFAULT_SYNC_MARKER.len() + 1] = 9;
    let (_, stats_codec) = read_all(bytes, ReaderConfig::default());
    assert_eq!(stats_codec.blocks_ok, 0);
    assert_eq!(stats_codec.blocks_bad, 1);
}

#[test]
fn oversized_lengths_rejected_safely() {
    let mut bytes = write_records(WriterConfig::default(), 1);
    let offset = DEFAULT_SYNC_MARKER.len() + 12;
    bytes[offset..offset + 4].copy_from_slice(&(32_u32 * 1024 * 1024).to_le_bytes());
    let (_, stats) = read_all(bytes, ReaderConfig { max_compressed_len: 1024 * 1024, max_uncompressed_len: 1024 * 1024, ..ReaderConfig::default() });

    assert_eq!(stats.blocks_ok, 0);
    assert_eq!(stats.blocks_bad, 1);
}

#[test]
fn single_huge_record_stored_in_own_block() {
    let payload = vec![7u8; 8192];
    let mut writer =
        LogjetWriter::with_config(Cursor::new(Vec::new()), WriterConfig { block_target_size: 128, codec: Codec::Lz4, ..Default::default() });
    writer.push(RecordType::Logs, 1, 10, b"small").unwrap();
    writer.push(RecordType::Logs, 2, 20, &payload).unwrap();
    writer.push(RecordType::Logs, 3, 30, b"tail").unwrap();
    let bytes = writer.into_inner().unwrap().into_inner();

    let sync_positions: Vec<usize> =
        bytes.windows(DEFAULT_SYNC_MARKER.len()).enumerate().filter_map(|(idx, window)| (window == DEFAULT_SYNC_MARKER).then_some(idx)).collect();
    assert_eq!(sync_positions.len(), 3);

    let (records, _) = read_all(bytes, ReaderConfig::default());
    assert_eq!(records.len(), 3);
    assert_eq!(records[1].3.len(), payload.len());
}

#[test]
fn seq_and_timestamp_deltas_round_trip() {
    let mut writer =
        LogjetWriter::with_config(Cursor::new(Vec::new()), WriterConfig { block_target_size: 1024, codec: Codec::None, ..Default::default() });
    writer.push(RecordType::Logs, 42, 1_000, b"a").unwrap();
    writer.push(RecordType::Metrics, 50, 1_250, b"b").unwrap();
    writer.push(RecordType::Traces, 99, 9_999, b"c").unwrap();
    let bytes = writer.into_inner().unwrap().into_inner();

    let (records, _) = read_all(bytes, ReaderConfig::default());
    assert_eq!(records[0].1, 42);
    assert_eq!(records[1].1, 50);
    assert_eq!(records[2].1, 99);
    assert_eq!(records[0].2, 1_000);
    assert_eq!(records[1].2, 1_250);
    assert_eq!(records[2].2, 9_999);
}
