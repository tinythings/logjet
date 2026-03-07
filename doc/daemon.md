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

- TCP ingest
- TCP replay
- in-memory ring buffering
- `.logjet` file output with rotation
- YAML config

What does not exist yet:

- OTLP/gRPC ingest
- OTLP/HTTP ingest
- downstream OTLP export client
- client acknowledgement protocol
- reconnect resume from saved cursor
