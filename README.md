# logjet

`logjet` is a compact append-only binary log format and Rust library for storing
and replaying raw OTLP protobuf batches on unreliable storage.

It is designed for telemetry relay and local persistence on weak hardware where:

- files may grow large
- writes must stay simple and fast
- reads must be sequential and streamable
- corruption may happen in the middle of a file
- later valid data must still be recoverable

## What It Is

`logjet` is:

- a block-based on-disk container format
- a Rust library for appending telemetry records and replaying them later
- a corruption-tolerant sequential reader with forward resynchronisation
- a transport-neutral storage layer for opaque OTLP protobuf payload bytes

Each record stores:

- a record type: logs, metrics, or traces
- a sequence number
- a Unix timestamp in nanoseconds
- the raw OTLP protobuf bytes

## How It Works

The file is an append-only sequence of independently verifiable blocks.

Each block contains:

1. an 8-byte sync marker
2. a fixed header
3. a small header extension with block base sequence and base timestamp
4. a payload containing multiple concatenated records
5. a trailing CRC32C checksum

Record payloads inside the block are stored as:

- `record_type: u8`
- `seq_delta: unsigned varint`
- `ts_delta_ns: unsigned varint`
- `payload_len: unsigned varint`
- `payload: raw OTLP protobuf bytes`

Important format properties:

- fixed-width integers are little-endian
- blocks are compressed independently
- default compression is LZ4
- `none` is also supported
- CRC32C is computed per block
- recovery works by scanning for the next sync marker after a bad block
- a reader never needs to load the whole file into memory

If a block is corrupted:

1. the reader detects the failure while validating the header, lengths, codec, or CRC
2. it treats that candidate block as bad
3. it resumes scanning from the next byte after the rejected sync marker
4. once another valid block is found, replay continues

That recovery model is the main reason the format is block-oriented instead of
using whole-file compression or per-record checksums.

## What It Is Not

`logjet` is not:

- a general-purpose database
- an indexed random-access storage engine
- a query layer for telemetry
- an OTLP decoder or validator
- a replacement for object storage, Kafka, or long-term analytics systems
- a whole-file archival compressor optimised for maximum compression ratio

It stores opaque OTLP protobuf bytes and focuses on durable append and reliable
sequential replay, not schema inspection or analytical querying.

## Library Usage

Add it to your project:

```toml
[dependencies]
logjet = { path = "../logjet" }
```

Write telemetry batches:

```rust
use std::fs::File;
use std::io::BufWriter;

use logjet::{LogjetWriter, RecordType};

fn persist_batches() -> Result<(), Box<dyn std::error::Error>> {
    let file = File::create("telemetry.logjet")?;
    let writer = BufWriter::new(file);
    let mut log = LogjetWriter::new(writer);

    let otlp_logs: Vec<u8> = vec![0x0a, 0x03, 0x66, 0x6f, 0x6f];
    let otlp_metrics: Vec<u8> = vec![0x12, 0x03, 0x62, 0x61, 0x72];

    log.push(RecordType::Logs, 1, 1_700_000_000_000_000_000, &otlp_logs)?;
    log.push(RecordType::Metrics, 2, 1_700_000_000_000_000_500, &otlp_metrics)?;

    let mut writer = log.into_inner()?;
    use std::io::Write;
    writer.flush()?;
    Ok(())
}
```

Replay them later:

```rust
use std::fs::File;
use std::io::BufReader;

use logjet::LogjetReader;

fn replay_batches() -> Result<(), Box<dyn std::error::Error>> {
    let file = File::open("telemetry.logjet")?;
    let mut reader = LogjetReader::new(BufReader::new(file));

    while let Some(record) = reader.next_record()? {
        println!(
            "type={:?} seq={} ts={} payload_len={}",
            record.record_type,
            record.seq,
            record.ts_unix_ns,
            record.payload.len()
        );

        // Forward the raw OTLP protobuf bytes to another system here.
        let _payload = record.payload;
    }

    let stats = reader.stats();
    println!(
        "blocks_ok={} blocks_bad={} bytes_skipped={} records_ok={}",
        stats.blocks_ok, stats.blocks_bad, stats.bytes_skipped, stats.records_ok
    );

    Ok(())
}
```

## Notes

- Examples for standalone usage live in [examples](./examples).
- The reader is sequential by design.
- Compression is per block, not per file.
- The payload bytes are opaque to `logjet`.
