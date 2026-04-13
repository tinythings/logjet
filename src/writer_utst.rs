use crate::{Codec, LogjetReader, LogjetWriter, RecordType, WriterConfig};
use std::io::Cursor;

fn round_trip(codec: Codec, payloads: &[Vec<u8>]) {
    let config = WriterConfig { codec, ..Default::default() };
    let mut writer = LogjetWriter::with_config(Vec::<u8>::new(), config);
    let base_ts = 1_700_000_000_000_000_000u64;

    for (i, payload) in payloads.iter().enumerate() {
        writer.push(RecordType::Logs, i as u64, base_ts + i as u64, payload).unwrap();
    }
    let data = writer.into_inner().unwrap();

    let cursor = Cursor::new(data);
    let mut reader = LogjetReader::new(cursor);
    for (i, expected) in payloads.iter().enumerate() {
        let record = reader.next_record().unwrap().unwrap_or_else(|| panic!("missing seq={i}"));
        assert_eq!(record.seq, i as u64, "seq mismatch at {i}");
        assert_eq!(
            record.payload.len(),
            expected.len(),
            "payload LENGTH mismatch at seq={i}: wrote {} read {}",
            expected.len(),
            record.payload.len(),
        );
        assert_eq!(record.payload, *expected, "payload CONTENT mismatch at seq={i}");
    }
    assert!(reader.next_record().unwrap().is_none(), "unexpected trailing record");
    assert_eq!(reader.stats().blocks_bad, 0);
}

/// Payloads at every nasty size boundary — varint edges, block boundaries, huge.
fn harsh_payloads() -> Vec<Vec<u8>> {
    let sizes: Vec<usize> = vec![
        // Tiny
        0, 1, 2, 7, 8, 15, 16, // Varint boundaries
        126, 127, 128, 129, 254, 255, 256, 257, // Larger varint boundaries
        4095, 4096, 4097, 8191, 8192, 8193, 16383, 16384, 16385, 32767, 32768, 32769, // Block target boundary (64 KiB)
        65534, 65535, 65536, 65537, // Sizes from the actual failure: 350, 477
        349, 350, 351, 476, 477, 478, // Medium realistic
        100, 200, 300, 500, 800, 1000, 1500, 2000, 3000, 4000, 5000, // Large realistic payloads
        10000, 20000, 40000,  // Near double-block
        130000, // Mix of all zeros, all 0xFF, protobuf-like patterns
        64, 64, 64, 64,
    ];

    sizes
        .iter()
        .enumerate()
        .map(|(i, &size)| {
            let mut buf = vec![0u8; size];
            match i % 5 {
                0 => {
                    // Protobuf-like: field tags + varint lengths
                    for (j, b) in buf.iter_mut().enumerate() {
                        *b = [0x0a, 0xda, 0x03, 0x12, 0xbe, 0x03, 0x09, 0x80][j % 8];
                    }
                }
                1 => buf.fill(0xFF),
                2 => buf.fill(0x00),
                3 => {
                    // High-entropy pseudorandom
                    for (j, b) in buf.iter_mut().enumerate() {
                        *b = (i.wrapping_mul(131) ^ j.wrapping_mul(251)) as u8;
                    }
                }
                _ => {
                    // ASCII text (syslog-like)
                    for (j, b) in buf.iter_mut().enumerate() {
                        *b = b"the quick brown fox jumps over the lazy dog 0123456789\n"[j % 54];
                    }
                }
            }
            buf
        })
        .collect()
}

#[test]
fn harsh_round_trip_none() {
    round_trip(Codec::None, &harsh_payloads());
}

#[test]
fn harsh_round_trip_lz4() {
    round_trip(Codec::Lz4, &harsh_payloads());
}

#[test]
fn harsh_round_trip_zstd() {
    round_trip(Codec::Zstd, &harsh_payloads());
}
