# logjetd Daemon

`logjetd` is a separate binary crate under [`logjetd/`](../logjetd).

## What It Does

`logjetd` accepts a simple TCP-framed record stream on an ingest socket and
replays backlog plus live records on a replay socket.

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

## Runtime Behavior

### Ingest

- binds to `ingest.listen`
- accepts TCP clients
- reads framed records
- appends them into configured storage

### Replay

- binds to `replay.listen`
- accepts TCP clients
- replays retained records in sequence order
- continues polling for newly ingested records

Replay is strictly sequential today. There is no checkpoint or resume token yet.

## Storage Modes

### `output: buffer`

- backlog exists only in memory
- storage is bounded by `buffer.size`
- if `buffer.preserve` is set, the first `N` retained messages are never evicted
- eviction removes later messages first, preserving the front of the buffer

Important:

- if the preserved prefix alone exceeds `buffer.size`, memory usage can exceed the configured size
- that is intentional because `buffer.preserve` is treated as a hard preservation rule

### `output: file`

- records are written to `.logjet` files
- files rotate when `file.size` is exceeded
- naming scheme is:
  - `bar.logjet`
  - `bar-1.logjet`
  - `bar-2.logjet`

Current file mode is rotating output, not a bounded on-disk ring.

## Current Limits

What exists now:

- TCP ingest
- TCP replay
- in-memory ring buffering
- `.logjet` file output with rotation
- YAML config

What does not exist yet:

- OTLP/gRPC ingest
- OTLP/HTTP ingest
- downstream OTLP export client
- disk-backed ring retention in file mode
- client acknowledgement protocol
- reconnect resume from saved cursor
