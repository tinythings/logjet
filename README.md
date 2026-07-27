# logjet

[![It is alive!](https://github.com/tinythings/logjet/actions/workflows/it-is-alive.yml/badge.svg)](https://github.com/tinythings/logjet/actions/workflows/it-is-alive.yml)
[![Unit Tests](https://github.com/tinythings/logjet/actions/workflows/unit-tests.yml/badge.svg)](https://github.com/tinythings/logjet/actions/workflows/unit-tests.yml)
[![Integration Tests](https://github.com/tinythings/logjet/actions/workflows/integration-tests.yml/badge.svg)](https://github.com/tinythings/logjet/actions/workflows/integration-tests.yml)
[![Insanity Check](https://github.com/tinythings/logjet/actions/workflows/insanity-check.yml/badge.svg)](https://github.com/tinythings/logjet/actions/workflows/insanity-check.yml)
[![Licence: Apache-2.0](https://img.shields.io/badge/licence-Apache--2.0-blue.svg)](./LICENSE)

`logjet` is a block-oriented binary format and Rust library for storing raw OTLP protobuf batches as an append-only stream. The project is aimed at telemetry relay, local backlog retention, and later replay on systems where sequential I/O is far easier to afford than elaborate indexing, background compaction, or large in-memory state. It is also intended for the less well-behaved parts of real deployments: links that are intermittent, slow, lossy, or simply unavailable for long enough that local backlog ceases to be optional. The emphasis throughout is on predictable writes, bounded reader memory, and partial recovery after corruption.

The repository contains more than the format crate itself. The `logjet` crate provides the storage format and the reader and writer APIs.

- `ljd` is a daemon for ingest, retention, replay, bridge mode, and file replay.
- `ljx` is the offline CLI for inspection, viewing, filtering, and export.
- `liblogjet` exposes the relevant pieces through a C ABI for C and C++ callers. The demos are not merely decorative; they are small executable scenarios for retention, replay, transport, plugin loading, TLS, and exporter integration.

The storage model is intentionally transport-neutral. A stored record carries a record type, a sequence number, a Unix timestamp in nanoseconds, and the raw OTLP payload bytes. The format does not interpret those bytes beyond preserving them faithfully. That boundary is deliberate. `logjet` is meant to be a reliable persistence and replay layer, not a query engine or a general telemetry warehouse.

## Format Overview

A `.logjet` file is an append-only sequence of independently verifiable blocks. Each block begins with a sync marker, followed by a fixed header, a small extension containing the block base sequence and base timestamp, a payload region containing multiple records, and a trailing CRC32C checksum. Fixed-width integers are little-endian. Compression is applied per block rather than per file. LZ4 is the default codec, and `none` is supported when compression is undesirable.

Within a block, records are encoded compactly through deltas relative to the block base sequence and timestamp. The essential fields are the record type, the sequence delta, the timestamp delta in nanoseconds, the payload length, and the raw payload itself. Sequence and timestamp deltas are stored as unsigned varints. This keeps the common case compact while preserving a reader that can operate in a strictly sequential manner.

The format is organised around recovery rather than random access. If a block is damaged, the reader validates headers, lengths, codec information, and checksum, rejects the invalid block, and resumes scanning for the next sync marker. Once another valid block is found, replay can continue from that point. This is the principal reason the design is block-based. Whole-file compression would make later valid data difficult to recover, and per-record framing would push overhead in the wrong direction for the intended workload.

The result is a format suited to persistent telemetry staging on unreliable media or modest hardware. It does not attempt to provide indexing, ad hoc search, or analytical execution. Those concerns belong elsewhere in the stack.

## Using The Rust Library

If you want to depend on the crate straight from the repository, point Cargo at GitHub:

```toml
[dependencies]
logjet = { git = "https://github.com/tinythings/logjet.git" }
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

## The Rest Of The Toolkit

The format crate is only one part of the repository. `ljd` handles live ingest and retained replay. It accepts OTLP/HTTP and OTLP/gRPC log traffic, supports plugin-based ingest, keeps backlog either in memory or in rotating `.logjet` segments, exposes a replay listener for downstream consumers, and can operate as a bridge to another collector. `ljx` covers the offline path: inspection, viewing, filtering, and export. `liblogjet` exists for environments in which the surrounding software is written in C or C++ and wants a stable library boundary rather than a Rust crate dependency.

The demos are useful as small reference systems. They show file-backed retention, memory retention, late consumer replay, replay handoff, bridge resume, TLS on replay links, plugin-based ingest, Parquet export through the external exporter ABI, and shared-library use from C++. In practice they also serve as executable documentation for behavior that is easier to understand by observing a working scenario than by reading a static paragraph.

## Build And Explore

From the project root, `make` builds the main release binaries. `make demo` builds the demo artefacts. `make test` runs the full test path, and `make check` runs clippy through the Makefile. Readers who prefer orientation before compilation should begin with [doc/index.md](./doc/index.md), then continue to [doc/overview.md](./doc/overview.md) for the system shape, [doc/configuration.md](./doc/configuration.md) for the YAML configuration surface, [doc/features.md](./doc/features.md) for the daemon feature set, and [doc/c-cpp-integration.md](./doc/c-cpp-integration.md) for the shared-library boundary. Standalone Rust examples remain in [examples](./examples), and the scenario-driven material remains in [demo](./demo).

## Final Word

The guiding preference in `logjet` is restraint. The format is compact, the reader is sequential, the recovery strategy is explicit, and the surrounding tools stay close to the operational problems they are meant to solve. That makes the project less ornate than many telemetry systems, but substantially easier to reason about when persistence, replay, and failure handling matter more than decorative complexity.
