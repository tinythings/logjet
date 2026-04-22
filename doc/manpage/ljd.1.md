% LJD(1)
% Bo Maryniuk
% March 2026

# NAME

ljd - OTLP ingest, `.logjet` storage, replay, and file blasting daemon

# SYNOPSIS

`ljd` [`serve`] [`-c`|`--config` *path*]

`ljd` `inspect` *path*

`ljd` `segments` `--path` *dir* `--name` *base.logjet*

`ljd` `replay` [`-c`|`--config` *path*] `--path` *dir* `--name` *base.logjet* [`--dest` *url-or-host:port*]

`ljd` `prune` `--path` *dir* `--name` *base.logjet* [`--keep-files` *n* | `--keep-bytes` *bytes*] [`--dry-run`]

`ljd` `bridge` [`-c`|`--config` *path*] [`--source` *host:port*]

# DESCRIPTION

`ljd` is a small telemetry daemon for constrained systems.

It can:

- accept OTLP/HTTP log batches on `POST /v1/logs`
- accept OTLP/gRPC log batches on the standard `LogsService/Export` endpoint
- optionally run OTLP/HTTP ingest over HTTPS and OTLP/gRPC ingest over TLS
- store raw OTLP protobuf payloads either in memory or in append-only `.logjet` files
- expose a replay listener for downstream consumers over the current internal wire protocol
- connect to another `ljd` replay listener and forward backlog plus live records into OTLP collectors
- optionally protect replay and bridge transport with TLS
- optionally export to an HTTPS OTLP collector
- export to a plain OTLP/gRPC collector
- inspect `.logjet` files and directories
- list file-segment metadata for one rotated spool
- replay stored `.logjet` files into OTLP collectors as a one-shot operation
- prune oldest rotated file segments by file count or byte budget

The daemon is designed for cheap hardware, limited RAM, unreliable storage, and
intermittent downstream connectivity.

# COMMANDS

## serve

Run the daemon using configuration loaded from `/etc/logjet.conf` by default, or
from the file passed with `-c` or `--config`.

If no explicit command is given, `serve` is the default.

Current serve behaviour:

- OTLP/HTTP ingest is supported with `ingest.protocol: otlp-http`
- OTLP/gRPC ingest is supported with `ingest.protocol: otlp-grpc`
- a replay listener socket is exposed for downstream clients
- replay sends retained backlog first, then hands the same client directly into live wakeups from ingest
- replay keeps an explicit cursor per client across buffer eviction, drain cleanup, and file rotation
- replay listener traffic currently uses the internal framed protocol, not OTLP egress

## bridge

Connect to another `ljd` replay listener, drain retained backlog, stay
attached for live records, and forward OTLP log payloads to the configured
collector.

Example:

```text
ljd --config ./logjetd.conf bridge --source 10.0.0.15:7002
```

If `--source` is omitted, bridge uses `upstream.replay` from configuration.

## inspect

Inspect a `.logjet` file or a directory containing `.logjet` files.

Example:

```text
ljd inspect /var/lib/logjet
```

## segments

List ordered rotated segments for one spool.

Example:

```text
ljd segments --path /var/lib/logjet --name app.logjet
```

## replay

Read ordered `.logjet` files from a directory and blast the stored OTLP log
payloads into OTLP collectors.

Example:

```text
ljd replay --path /var/logs --name app.logjet --dest http://127.0.0.1:4318/v1/logs
```

Replay order is:

- `app.logjet`
- `app-1.logjet`
- `app-2.logjet`
- and so on

Replay is immediate and does not preserve original timing.
If `--dest` is omitted, replay uses `collector.url` from configuration.

## prune

Remove oldest rotated file segments deliberately.

Examples:

```text
ljd prune --path /var/lib/logjet --name app.logjet --keep-files 2
ljd prune --path /var/lib/logjet --name app.logjet --keep-bytes 1048576 --dry-run
```

# OPTIONS

## `-c`, `--config` *path*

Load configuration from *path* instead of `/etc/logjet.conf`.

## `-h`, `--help`

Print usage information.

## `--path` *dir*

Directory containing `.logjet` files for replay.

Used with the `segments`, `replay`, and `prune` commands.

## `--name` *base.logjet*

Base file name used to locate replay segments.

Example:

- `app.logjet`
- `app-1.logjet`
- `app-2.logjet`

Used only with the `replay` command.

## `--keep-files` *n*

Keep only the newest *n* segment files during `prune`.

## `--keep-bytes` *bytes*

Keep only the newest segment set that fits within *bytes* during `prune`.

## `--dry-run`

Show which files `prune` would remove without deleting them.

## `--dest` *url-or-host:port*

Destination OTLP collector for replay.

Used only with the `replay` command.

If a full `http://host:port/path` URL is given, replay uses that exact path.
If `https://` is given, replay uses OTLP/HTTP over TLS.
If `grpc://host:port` is given, replay uses OTLP/gRPC.
If only `host:port` is given, replay defaults to `/v1/logs`.

## `--source` *host:port*

Upstream `ljd` replay listener for the `bridge` command.

If omitted, `bridge` uses `upstream.replay` from configuration.

# CONFIGURATION

Default configuration path:

```text
/etc/logjet.conf
```

Important keys:

```yaml
output: buffer
buffer.size: 100
buffer.messages: 5000
buffer.keep: 1500
file.path: /var/lib/logjet
file.size: 10240
file.name: vehicle.logjet
collector.url: http://127.0.0.1:4318/v1/logs
collector.timeout-ms: 10000
collector.ca-file: /etc/logjet/collector-ca.pem
collector.cert-file: /etc/logjet/collector.pem
collector.key-file: /etc/logjet/collector.key
collector.server-name: collector.internal
backpressure.enabled: false
backpressure.mode: disconnect
backpressure.max-buffered-records: 16
upstream.replay: 10.0.0.15:7002
upstream.mode: keep
upstream.state-file: /var/lib/logjet/bridge.state
upstream.retry-ms: 1000
upstream.connect-timeout-ms: 5000
tls.enable: false
tls.ca-file: /etc/logjet/ca.pem
tls.cert-file: /etc/logjet/node.pem
tls.key-file: /etc/logjet/node.key
tls.require-client-cert: false
tls.server-name: appliance.internal
ingest.protocol: otlp-http
ingest.listen: 127.0.0.1:4318
ingest.tls-enable: false
ingest.ca-file: /etc/logjet/ingest-ca.pem
ingest.cert-file: /etc/logjet/ingest.pem
ingest.key-file: /etc/logjet/ingest.key
ingest.require-client-cert: false
ingest.max-batch-bytes: 1048576
ingest.max-clients: 32
ingest.max-batches-per-second: 0
ingest.priority-severity-at-least: error
ingest.overload-report-ms: 5000
replay.listen: 0.0.0.0:7002
replay.max-clients: 32
replay.client-timeout-ms: 10000
```

Rules:

- `buffer.size` is measured in KiB
- `file.size` is measured in KiB
- set either `buffer.size` or `buffer.messages`, never both
- `buffer.keep` applies only to memory mode
- file mode always keeps all rotated files
- `segments` and `prune` provide explicit operator tooling for file-mode archive housekeeping
- `ingest.protocol` supports `wire`, `otlp-http`, and `otlp-grpc`
- `ingest.max-batch-bytes` rejects oversized OTLP or wire payloads before they are stored
- `ingest.max-clients` caps concurrent ingest handling
- `ingest.max-batches-per-second` caps accepted ingest batches per second
- `ingest.priority-severity-at-least` lets higher-severity OTLP log batches bypass overload shedding
- `ingest.overload-report-ms` controls operator-visible overload summaries on stderr
- `replay.max-clients` caps concurrent replay clients
- `replay.client-timeout-ms` caps how long one replay client can block on socket I/O
- `collector.url` configures bridge and replay destination URL or URL list
- `collector.timeout-ms` configures replay and bridge socket timeout in milliseconds
- `collector.ca-file`, `collector.cert-file`, `collector.key-file`, and `collector.server-name` configure `https://...` and `grpcs://...` collector export
- one collector TLS config is shared across all TLS collector destinations in one process
- mixed plain plus TLS fan-out is supported
- `grpcs://...` with only `collector.ca-file` is plain TLS with server validation
- `grpcs://...` with `collector.cert-file` and `collector.key-file` adds mutual TLS client authentication
- if one TLS destination fails handshake or export, that batch fails for the whole fan-out set
- different TLS trust roots, client certs, or server-name overrides require separate `ljd` instances
- `backpressure.enabled` enables bridge backpressure policy handling
- `backpressure.mode` configures whether bridge export blocks, disconnects, or drops newest records when the collector is too slow
- `backpressure.max-buffered-records` caps the bridge-side exporter queue per bridge connection
- `upstream.replay` configures the default bridge source
- `upstream.mode` configures whether bridge keeps or drains upstream retained records
- `upstream.state-file` stores persisted bridge resume state
- `upstream.retry-ms` configures bridge reconnect delay
- `upstream.connect-timeout-ms` configures bridge source connect timeout
- `ingest.*` configures optional TLS on OTLP/HTTP and OTLP/gRPC ingest
- `tls.*` configures optional TLS for replay listener and bridge transport

# STORAGE MODES

## Buffer mode

In-memory ring behaviour:

- first `buffer.keep` messages are retained in a permanent front jar
- later messages form a rotating FIFO tail

Drain model:

```text
[kept messages][rotating tail]
```

## File mode

Append-only file behaviour:

- write to `name.logjet`
- rotate to `name-1.logjet`, `name-2.logjet`, and so on when `file.size` is exceeded
- old rotated files are kept

# CURRENT FEATURES

- OTLP/HTTP log ingest on `POST /v1/logs`
- OTLP/gRPC log ingest on the standard logs export service
- optional TLS on OTLP/HTTP and OTLP/gRPC ingest
- basic ingest guardrails through `ingest.max-batch-bytes` and `ingest.max-clients`
- ingest rate limiting with severity-aware overload shedding
- append-only `.logjet` file output with size-based rotation
- in-memory ring buffering with `buffer.keep`
- replay listener for downstream consumers using the internal framed protocol
- independent replay cursor per connected client
- backlog-to-live replay handoff through direct ingest wakeups
- basic replay-client caps through `replay.max-clients`
- basic replay-client timeout through `replay.client-timeout-ms`
- continuous bridge mode from replay listener to OTLP collectors
- acknowledged drain mode through `upstream.mode: drain`
- persisted bridge resume through `upstream.state-file`
- upstream restart and storage-replacement detection through replay stream identity
- basic backpressure policy through `backpressure.mode`
- bounded bridge-side exporter queue through `backpressure.max-buffered-records`
- optional TLS on replay/bridge transport
- HTTPS OTLP collector export
- plain OTLP/gRPC collector export
- one-shot file replay to OTLP collectors with `ljd replay`
- configurable replay destination via `collector.url`
- inspection of `.logjet` files and directories

# CURRENT LIMITATIONS

- replay listener traffic is not OTLP
- upstream storage reset and rollover handling is still basic
- ingest overload handling is limited to payload-size caps and concurrent-client caps
- multi-client replay isolation is still basic beyond per-client cursors, wakeups, and replay-client caps
- file mode does not delete old rotated files
- certificate management and deployment policy are still operator-managed

# EXAMPLES

Run the daemon with explicit config:

```text
ljd --config ./logjetd.conf
```

Inspect generated files:

```text
ljd inspect ./logs
```

Replay files to a local collector:

```text
ljd --config ./logjetd.conf replay --path ./logs --name app.logjet
```

Bridge from a remote replay listener into a collector:

```text
ljd --config ./logjetd.conf bridge --source 10.0.0.15:7002
```

# FILES

`/etc/logjet.conf`
: default configuration file

`*.logjet`
: append-only telemetry files written by file mode

`doc/manpage/ljd.1.md`
: Markdown manpage source

`doc/manpage/ljd.1`
: generated manpage output

# EXIT STATUS

`0`
: success

non-zero
: error

# SEE ALSO

`pandoc`(1)
