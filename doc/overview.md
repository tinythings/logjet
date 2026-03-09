# logjet Overview

`logjet` is split into two parts:

- `logjet`: a Rust library and `.logjet` block format for storing raw OTLP protobuf batches
- `ljd`: a daemon that accepts OTLP logs, keeps a backlog, and replays or blasts stored data later

## Components

### `logjet`

The library provides:

- append-only block writer
- sequential recovery reader
- corruption tolerance through sync markers and per-block CRC32C
- block-local compression with `lz4` or `none`

### `ljd`

The daemon provides:

- OTLP/HTTP ingest listener
- OTLP/gRPC ingest listener
- optional TLS for OTLP ingest
- internal wire-protocol ingest listener
- replay listener
- per-client replay cursors
- continuous bridge mode to another collector through a remote replay listener
- configurable keep-or-drain bridge semantics
- optional persisted bridge resume state
- basic backpressure policy on bridge export
- bounded bridge-side exporter queue with `block`, `disconnect`, and `drop-newest` policy
- basic ingest guardrails for payload size and concurrent clients
- ingest rate limiting with severity-aware overload shedding
- basic replay-client caps
- optional TLS on the replay/bridge transport
- HTTPS collector export
- bridge-side restart recovery through persisted sequence state and upstream stream identity
- one-shot file replay to OTLP/HTTP collectors
- configurable backlog storage
- in-memory ring buffer mode
- file output mode with `.logjet` segment rotation
- file segment inspection and pruning commands for explicit archive housekeeping

## Intended Use

`ljd` is meant to sit next to a local telemetry source such as an OTel
Appliance (`OA`).

Typical flow:

1. a local source connects to the ingest listener
2. `ljd` stores records in memory or `.logjet` files
3. a downstream consumer connects to the replay listener
4. `ljd` sends retained backlog first
5. `ljd` continues sending newly ingested records

`ljd` can also run remotely in bridge mode:

1. connect to another `ljd` replay listener
2. request records after a known sequence
3. keep or drain retained backlog
4. stay attached for live records
5. forward raw OTLP protobuf payloads into an OTLP/HTTP collector

`ljd` can also replay stored `.logjet` files later into an OTLP/HTTP
collector without preserving original timing.

This is optimised for:

- weak CPUs
- limited RAM
- unreliable storage
- intermittent downstream connectivity
- sequential replay instead of random-access querying
