# logjet Overview

`logjet` is split into two parts:

- `logjet`: a Rust library and `.logjet` block format for storing raw OTLP protobuf batches
- `logjetd`: a daemon that accepts OTLP logs, keeps a backlog, and replays or blasts stored data later

## Components

### `logjet`

The library provides:

- append-only block writer
- sequential recovery reader
- corruption tolerance through sync markers and per-block CRC32C
- block-local compression with `lz4` or `none`

### `logjetd`

The daemon provides:

- OTLP/HTTP ingest listener
- OTLP/gRPC ingest listener
- internal wire-protocol ingest listener
- replay listener
- one-shot file replay to OTLP/HTTP collectors
- configurable backlog storage
- in-memory ring buffer mode
- file output mode with `.logjet` segment rotation

## Intended Use

`logjetd` is meant to sit next to a local telemetry source such as `traceserver`.

Typical flow:

1. a local source connects to the ingest listener
2. `logjetd` stores records in memory or `.logjet` files
3. a downstream consumer connects to the replay listener
4. `logjetd` sends retained backlog first
5. `logjetd` continues sending newly ingested records

`logjetd` can also replay stored `.logjet` files later into an OTLP/HTTP
collector without preserving original timing.

This is optimized for:

- weak CPUs
- limited RAM
- unreliable storage
- intermittent downstream connectivity
- sequential replay instead of random-access querying
