# logjetd Features

This file tracks the current feature set and practical use cases of `logjetd`.
It is meant to evolve as the daemon grows.

## Current Features

### 1. OTLP log ingest

`logjetd` can accept real OTLP/HTTP protobuf log export requests on:

```text
POST /v1/logs
```

It can also accept OTLP/gRPC log export requests on the standard
`LogsService/Export` endpoint when `ingest.protocol: otlp-grpc` is configured.

Current behavior:

- accepts OTLP log batches over HTTP and gRPC
- validates that the request decodes as `ExportLogsServiceRequest`
- stores the raw OTLP protobuf bytes
- assigns a local sequence number for internal replay ordering

### 2. Append-only `.logjet` file output

In file mode, `logjetd` writes raw OTLP protobuf batches into `.logjet` files
using the `logjet` block format.

Current behavior:

- append-only writes
- file rotation when `file.size` is exceeded
- rotated files are kept
- naming style:
  - `name.logjet`
  - `name-1.logjet`
  - `name-2.logjet`

### 3. In-memory ring buffer mode

In buffer mode, `logjetd` can hold retained records in RAM.

Current behavior:

- supports byte-based limit with `buffer.size`
- supports message-count limit with `buffer.messages`
- `buffer.size` and `buffer.messages` are mutually exclusive
- supports `buffer.keep` to permanently retain the first `N` messages
- `buffer.size` and `buffer.messages` apply to the rotating tail only

Memory model:

- front jar: first `buffer.keep` messages are never evicted
- rotating tail: later messages are evicted FIFO-style
- total retained = kept front jar + rotating tail

### 4. Replay listener

`logjetd` exposes a replay socket for downstream consumers.

Current behavior:

- replays retained data in sequence order
- continues polling for new records
- supports multiple clients in a basic way

Current limitation:

- replay uses a custom internal wire protocol, not OTLP egress yet

### 5. One-shot file replay to OTLP/HTTP

`logjetd` can replay stored `.logjet` files directly into an OTLP/HTTP
collector with:

```text
logjetd replay --path <dir> --name <base.logjet> [--dest <url-or-host:port>]
```

Current behavior:

- scans for `name.logjet`, `name-1.logjet`, `name-2.logjet`, and so on
- replays them in that order
- reads stored `logs` records
- posts the raw OTLP protobuf payloads to `collector.url`
- sends as fast as the destination socket allows, with no artificial delay
- if `--dest` is omitted, replay uses `collector.url` from config

### 6. YAML configuration

`logjetd` reads configuration from:

- `/etc/logjet.conf` by default
- a file passed with `-c` or `--config`

Current config areas:

- output mode
- in-memory buffer sizing
- file rotation sizing
- ingest protocol
- ingest and replay bind addresses
- replay polling interval
- collector URL and timeout

### 7. Inspection tooling

`logjetd` can inspect stored `.logjet` files or directories and print metadata
about retained records.

## Current Use Cases

### 1. Local OTLP capture to files

Use case:

- an emitter sends OTLP logs locally
- `logjetd` stores them in `.logjet` files
- files can be inspected, extracted, or replayed later

Useful when:

- the device cannot forward immediately
- local persistence matters more than live export

### 2. Boot-time message retention in RAM

Use case:

- the appliance starts producing logs very early
- the downstream collector connects too late
- `buffer.keep` preserves the first important startup messages

Useful when:

- boot diagnostics must not disappear
- later traffic can rotate normally

### 3. Lightweight telemetry staging on weak hardware

Use case:

- device hardware is slow and RAM is limited
- the daemon must stay simple
- storage must be sequential and cheap to write

Useful when:

- automotive and avionics targets are resource-constrained
- storage and networking are unreliable

### 4. Demo and lab setups without a full collector stack

Use case:

- use the OTLP demo emitter
- send logs into `logjetd`
- store them or inspect them locally

Useful when:

- a full Vector or collector deployment would be overkill
- you want to demonstrate end-to-end OTLP ingest quickly

### 5. Offline file replay into an OTLP collector

Use case:

- capture OTLP batches to `.logjet` files on one run or one machine
- later replay those files into an OTLP collector
- do that as fast as possible without waiting for original timings

Useful when:

- you want a fast demo
- you need bulk backfill of recorded OTLP logs
- you want to validate stored files against a collector pipeline

## Current Non-Features

These are not implemented yet:

- continuous OTLP egress from the daemon
- resume checkpoints or acknowledgements
- advanced slow-consumer handling
- disk-budget retention management for rotated files
- production-grade service lifecycle handling
