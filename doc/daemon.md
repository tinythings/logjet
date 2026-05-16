# ljd Daemon

`ljd` is a separate binary crate under [`logjetd/`](../logjetd).

## What It Does

`ljd` accepts telemetry on an ingest socket, stores backlog, and can replay
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
ljd
```

Explicit config path:

```bash
ljd --config /path/to/logjet.conf
```

List visible ingest and export plugins:

```bash
ljd --plugins
```

The plugin listing is plain text. Each plugin entry prints `name` and
`display-name` on one tab-indented line, followed by the plugin path on the
next line. If `--config` is provided, configured ingest plugin locations such
as `ingest.plugin-path` and `ingest.plugin-dir` are included in the ingest
scan.

Inspect a file or directory:

```bash
ljd inspect /var/lib/logjet
```

List rotated file segments for one spool:

```bash
ljd segments --path /var/lib/logjet --name app.logjet
```

Prune oldest rotated file segments and keep only the newest two files:

```bash
ljd prune --path /var/lib/logjet --name app.logjet --keep-files 2
```

Preview byte-budget pruning without deleting anything:

```bash
ljd prune --path /var/lib/logjet --name app.logjet --keep-bytes 1048576 --dry-run
```

Continuously bridge from another `ljd` replay listener into an OTLP
collector:

```bash
ljd --config /path/to/logjet.conf bridge --source 10.0.0.15:7002
```

## Runtime Behaviour

### Ingest

- binds to `ingest.listen`
- supports `ingest.protocol: wire`
- supports `ingest.protocol: otlp-http`
- supports `ingest.protocol: otlp-grpc`
- supports `ingest.protocol: plugin` with `ingest.plugin-path`
- supports TLS on OTLP/HTTP and OTLP/gRPC ingest with `ingest.*`
- can rate-limit accepted ingest batches through `ingest.max-batches-per-second`
- can keep higher-severity OTLP batches during overload through `ingest.priority-severity-at-least`
- stores raw OTLP protobuf bytes in configured storage

For plugin ingest, explicit `.so` values in `ingest.plugin-path` are loaded
directly. Bare filenames are resolved through `LJD_INGEST_PLUGIN_PATH`,
`./ingestors`, `<ljd bin>/ingestors`, `<ljd bin>/../lib/logjet/ingestors`, and
on Unix also `/usr/lib/logjet/ingestors` plus `/usr/lib/logjet`.

To select by plugin descriptor name, set `ingest.use` or `ingest.plugin`.
Then `ingest.plugin-path` may be a directory, or `ingest.plugin-dir` may name
the directory:

```yaml
ingest.protocol: plugin
ingest.plugin-path: /opt/plugins
ingest.use: logcat
```

### Replay

- binds to `replay.listen`
- caps concurrent replay clients through `replay.max-clients`
- bounds blocked replay socket I/O per client through `replay.client-timeout-ms`
- accepts TCP clients
- sends a replay hello carrying upstream stream identity before the replay request
- expects a small replay request carrying `from_seq`
- replay requests can ask to `keep` or `drain`
- replays retained records in sequence order
- uses a per-client replay cursor so retained backlog can hand off directly into live wakeups from ingest
- can optionally wrap the replay transport in TLS

Replay is strictly sequential today. Resume exists per connection via `from_seq`.
In `drain` mode, records are consumed only after the downstream side acknowledges
successful export. Bridge mode can also persist the last forwarded sequence
through `upstream.state-file`. If the upstream daemon restarts in buffer mode or
the upstream storage is replaced, the replay hello stream identity lets bridge
reset stale saved sequence state instead of getting stuck above the restarted
upstream.

### Continuous Bridge

- `ljd bridge` connects to another `ljd` replay listener
- requests either `keep` or `drain` mode from the upstream side
- continues forwarding newly replayed records
- forwards OTLP payloads to every destination configured in `collector.url`
- reconnects after disconnect using the last in-process forwarded sequence
- can also load and save the last forwarded sequence through `upstream.state-file`
- resets saved bridge sequence state automatically when upstream stream identity changes
- source address comes from `--source` or `upstream.replay`
- `upstream.mode: drain` removes upstream records after successful export to every configured collector destination and acknowledgement
- `backpressure.enabled: true` enables explicit bridge backpressure policy handling
- `backpressure.mode: disconnect` uses collector timeouts as a fail-fast policy
- `backpressure.mode: block` waits for the collector reply instead of timing out
- `backpressure.mode: drop-newest` keeps the bridge live and drops newest records when the export queue is full
- `backpressure.max-buffered-records` caps the bridge-side exporter queue per bridge connection
- can optionally use TLS with `tls.*`
- collector export can use OTLP/HTTP, HTTPS, and plain OTLP/gRPC
- `collector.*` TLS settings apply to HTTPS collector export

### File Blast Replay

- `ljd replay --path ... --name ...`
- reads ordered rotated `.logjet` files
- sends stored OTLP batches to the configured destination set
- uses `collector.url` by default
- `--dest` can override the collector destination
- if `collector.url` is a list, replay fans out to every configured destination
- sends as fast as possible, with no original timing preservation

### File Operational Tooling

- `ljd segments --path ... --name ...`
- prints ordered metadata for one rotated spool:
  - segment id
  - file path
  - file size in bytes
  - record count
  - first and last sequence
- `ljd prune --path ... --name ... --keep-files <n>`
- removes oldest rotated segments and keeps the newest `n` segment files
- `ljd prune --path ... --name ... --keep-bytes <bytes>`
- removes oldest rotated segments until the newest retained set fits within the byte budget
- `--dry-run` prints the paths that would be removed without deleting them
- the newest active segment is always retained

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

For deployments that keep rotated files, `segments` and `prune` provide the
operator-facing archive management step. This keeps daemon behaviour explicit:
file output stays append-only, and archive cleanup is a deliberate command.

## Current Limits

What exists now:

- wire-protocol ingest
- OTLP/HTTP ingest
- OTLP/gRPC ingest
- ingest overload policy with rate limiting and severity-aware shedding
- TCP replay
- per-client replay cursors
- basic replay-client caps
- continuous daemon-to-daemon bridge to OTLP collectors
- configurable keep-or-drain bridge mode
- basic bridge backpressure policy
- bounded bridge-side export queue
- optional TLS on replay/bridge transport
- TLS-enabled OTLP ingest
- HTTPS collector export
- plain OTLP/gRPC collector export
- one-shot file replay to OTLP collectors
- in-memory ring buffering
- `.logjet` file output with rotation
- YAML config

What does not exist yet:

- self-observability for exporter queue saturation and drop counts
