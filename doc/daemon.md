# logjetd Daemon

`logjetd` is a separate binary crate under [`logjetd/`](../logjetd).

## What It Does

`logjetd` accepts telemetry on an ingest socket, stores backlog, and can replay
that backlog later.

It supports two storage modes:

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

## Runtime Behaviour

### Ingest

- binds to `ingest.listen`
- supports `ingest.protocol: wire`
- supports `ingest.protocol: otlp-http`
- supports `ingest.protocol: otlp-grpc`
- supports TLS on OTLP/HTTP and OTLP/gRPC ingest with `ingest.*`
- stores raw OTLP protobuf bytes in configured storage

### Replay

- binds to `replay.listen`
- caps concurrent replay clients through `replay.max-clients`
- accepts TCP clients
- sends a replay hello carrying upstream stream identity before the replay request
- expects a small replay request carrying `from_seq`
- replay requests can ask to `keep` or `drain`
- replays retained records in sequence order
- continues polling for newly ingested records
- can optionally wrap the replay transport in TLS

Replay is strictly sequential today. Resume exists per connection via `from_seq`.
In `drain` mode, records are consumed only after the downstream side acknowledges
successful export. Bridge mode can also persist the last forwarded sequence
through `upstream.state-file`. If the upstream daemon restarts in buffer mode or
the upstream storage is replaced, the replay hello stream identity lets bridge
reset stale saved sequence state instead of getting stuck above the restarted
upstream.

### Continuous Bridge

- `logjetd bridge` connects to another `logjetd` replay listener
- requests either `keep` or `drain` mode from the upstream side
- continues forwarding newly replayed records
- forwards OTLP log payloads to `collector.url`
- reconnects after disconnect using the last in-process forwarded sequence
- can also load and save the last forwarded sequence through `upstream.state-file`
- resets saved bridge sequence state automatically when upstream stream identity changes
- source address comes from `--source` or `upstream.replay`
- `upstream.mode: drain` removes upstream records after successful collector export and acknowledgement
- `backpressure.enabled: true` enables explicit bridge backpressure policy handling
- `backpressure.mode: disconnect` uses collector timeouts as a fail-fast policy
- `backpressure.mode: block` waits for the collector reply instead of timing out
- can optionally use TLS with `tls.*`
- collector export can optionally use HTTPS with `collector.*`

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
- sequence numbering resumes from the highest stored sequence after restart
- files rotate when `file.size` is exceeded
- fully consumed closed files can be deleted when bridge mode uses `upstream.mode: drain`
- naming scheme is:
  - `bar.logjet`
  - `bar-1.logjet`
  - `bar-2.logjet`

Current file mode is append-only output with file rotation. In drain mode, closed
consumed segments can disappear after successful forwarding.

## Current Limits

What exists now:

- wire-protocol ingest
- OTLP/HTTP ingest
- OTLP/gRPC ingest
- TCP replay
- per-client replay cursors
- basic replay-client caps
- continuous daemon-to-daemon bridge to OTLP/HTTP collectors
- configurable keep-or-drain bridge mode
- basic bridge backpressure policy
- optional TLS on replay/bridge transport
- TLS-enabled OTLP ingest
- HTTPS collector export
- one-shot file replay to OTLP/HTTP collectors
- in-memory ring buffering
- `.logjet` file output with rotation
- YAML config

What does not exist yet:

- advanced restart handling when upstream storage is reset or replaced
- richer slow-consumer policy than `block` and `disconnect`
