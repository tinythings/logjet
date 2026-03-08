# logjetd Daemon

`logjetd` is a separate binary crate under [`logjetd/`](../logjetd).

## What It Does

`logjetd` accepts telemetry on an ingest socket, stores backlog, and can replay
that backlog later.

It currently supports two storage modes:

- `buffer`: in-memory ring buffer
- `file`: on-disk `.logjet` segment files

## Command Line

Default config path:

```text
/etc/logjet.conf
```

Run with default config path:

```bash
logjetd
```

Explicit config path:

```bash
logjetd --config /path/to/logjet.conf
```

Inspect a file or directory:

```bash
logjetd inspect /var/lib/logjet
```

Continuously bridge from another `logjetd` replay listener into an OTLP
collector:

```bash
logjetd --config /path/to/logjet.conf bridge --source 10.0.0.15:7002
```

## Runtime Behavior

### Ingest

- binds to `ingest.listen`
- supports `ingest.protocol: wire`
- supports `ingest.protocol: otlp-http`
- supports `ingest.protocol: otlp-grpc`
- stores raw OTLP protobuf bytes in configured storage

### Replay

- binds to `replay.listen`
- accepts TCP clients
- expects a small replay request carrying `from_seq`
- replays retained records in sequence order
- continues polling for newly ingested records

Replay is strictly sequential today. Resume exists per connection via `from_seq`,
but there is no persisted checkpoint yet.

### Continuous Bridge

- `logjetd bridge` connects to another `logjetd` replay listener
- drains retained backlog first
- continues forwarding newly replayed records
- forwards OTLP log payloads to `collector.url`
- reconnects after disconnect using the last in-process forwarded sequence
- source address comes from `--source` or `upstream.replay`

### File Blast Replay

- `logjetd replay --path ... --name ...`
- reads ordered rotated `.logjet` files
- sends stored OTLP log batches to a collector URL
- uses `collector.url` by default
- `--dest` can override the collector destination
- sends as fast as possible, with no original timing preservation

## Storage Modes

### `output: buffer`

- backlog exists only in memory
- storage is bounded by either `buffer.size` or `buffer.messages`
- if `buffer.keep` is set, the first `N` retained messages are never evicted
- think of this as two pools:
- a jar holding the first `N` messages forever
- a FIFO behind the jar that rotates normally
- eviction removes later messages first, preserving the jar at the front

Important:

- if the kept prefix alone exceeds the configured limit, memory usage or retained message count can exceed the rotating tail budget
- that is intentional because `buffer.keep` is treated as a hard preservation rule

### `output: file`

- records are written to `.logjet` files
- files rotate when `file.size` is exceeded
- old files are not deleted by `logjetd`
- file mode always keeps everything written
- naming scheme is:
  - `bar.logjet`
  - `bar-1.logjet`
  - `bar-2.logjet`

Current file mode is append-only output with file rotation, not a ring.

## Current Limits

What exists now:

- wire-protocol ingest
- OTLP/HTTP ingest
- OTLP/gRPC ingest
- TCP replay
- continuous daemon-to-daemon bridge to OTLP/HTTP collectors
- one-shot file replay to OTLP/HTTP collectors
- in-memory ring buffering
- `.logjet` file output with rotation
- YAML config

What does not exist yet:

- persisted reconnect resume from saved cursor
- client acknowledgement protocol
