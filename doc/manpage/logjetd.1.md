% LOGJETD(1)
% Bo Maryniuk
% March 2026

# NAME

logjetd - OTLP ingest, `.logjet` storage, replay, and file blasting daemon

# SYNOPSIS

`logjetd` [`serve`] [`-c`|`--config` *path*]

`logjetd` `inspect` *path*

`logjetd` `replay` [`-c`|`--config` *path*] `--path` *dir* `--name` *base.logjet* [`--dest` *url-or-host:port*]

`logjetd` `bridge` [`-c`|`--config` *path*] [`--source` *host:port*]

# DESCRIPTION

`logjetd` is a small telemetry daemon for constrained systems.

It can:

- accept OTLP/HTTP log batches on `POST /v1/logs`
- accept OTLP/gRPC log batches on the standard `LogsService/Export` endpoint
- optionally run OTLP/HTTP ingest over HTTPS and OTLP/gRPC ingest over TLS
- store raw OTLP protobuf payloads either in memory or in append-only `.logjet` files
- expose a replay listener for downstream consumers over the current internal wire protocol
- connect to another `logjetd` replay listener and forward backlog plus live records into an OTLP/HTTP collector
- optionally protect replay and bridge transport with TLS
- optionally export to an HTTPS OTLP collector
- inspect `.logjet` files and directories
- replay stored `.logjet` files into an OTLP/HTTP collector as a one-shot operation

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
- replay listener traffic currently uses the internal framed protocol, not OTLP egress

## bridge

Connect to another `logjetd` replay listener, drain retained backlog, stay
attached for live records, and forward OTLP log payloads to the configured
collector.

Example:

```text
logjetd --config ./logjetd.conf bridge --source 10.0.0.15:7002
```

If `--source` is omitted, bridge uses `upstream.replay` from configuration.

## inspect

Inspect a `.logjet` file or a directory containing `.logjet` files.

Example:

```text
logjetd inspect /var/lib/logjet
```

## replay

Read ordered `.logjet` files from a directory and blast the stored OTLP log
payloads into an OTLP/HTTP collector.

Example:

```text
logjetd replay --path /var/logs --name bofh.logjet --dest http://127.0.0.1:4318/v1/logs
```

Replay order is:

- `bofh.logjet`
- `bofh-1.logjet`
- `bofh-2.logjet`
- and so on

Replay is immediate and does not preserve original timing.
If `--dest` is omitted, replay uses `collector.url` from configuration.

# OPTIONS

## `-c`, `--config` *path*

Load configuration from *path* instead of `/etc/logjet.conf`.

## `-h`, `--help`

Print usage information.

## `--path` *dir*

Directory containing `.logjet` files for replay.

Used only with the `replay` command.

## `--name` *base.logjet*

Base file name used to locate replay segments.

Example:

- `bofh.logjet`
- `bofh-1.logjet`
- `bofh-2.logjet`

Used only with the `replay` command.

## `--dest` *url-or-host:port*

Destination OTLP/HTTP collector for replay.

Used only with the `replay` command.

If a full `http://host:port/path` URL is given, replay uses that exact path.
If only `host:port` is given, replay defaults to `/v1/logs`.

## `--source` *host:port*

Upstream `logjetd` replay listener for the `bridge` command.

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
collector.server-name: vector.internal
backpressure.enabled: false
backpressure.mode: disconnect
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
replay.listen: 0.0.0.0:7002
replay.poll_ms: 250
```

Rules:

- `buffer.size` is measured in KiB
- `file.size` is measured in KiB
- set either `buffer.size` or `buffer.messages`, never both
- `buffer.keep` applies only to memory mode
- file mode always keeps all rotated files
- `ingest.protocol` supports `wire`, `otlp-http`, and `otlp-grpc`
- `ingest.max-batch-bytes` rejects oversized OTLP or wire payloads before they are stored
- `ingest.max-clients` caps concurrent ingest handling
- `collector.url` configures replay destination URL
- `collector.timeout-ms` configures replay and bridge socket timeout in milliseconds
- `collector.ca-file`, `collector.cert-file`, `collector.key-file`, and `collector.server-name` configure HTTPS collector export
- `backpressure.enabled` enables bridge backpressure policy handling
- `backpressure.mode` configures whether bridge export blocks or disconnects when the collector is too slow
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
- append-only `.logjet` file output with size-based rotation
- in-memory ring buffering with `buffer.keep`
- replay listener for downstream consumers using the internal framed protocol
- continuous bridge mode from replay listener to OTLP/HTTP collectors
- acknowledged drain mode through `upstream.mode: drain`
- persisted bridge resume through `upstream.state-file`
- basic backpressure policy through `backpressure.mode`
- optional TLS on replay/bridge transport
- HTTPS OTLP collector export
- one-shot file replay to OTLP/HTTP collectors with `logjetd replay`
- configurable replay destination via `collector.url`
- inspection of `.logjet` files and directories

# CURRENT LIMITATIONS

- replay listener traffic is not OTLP
- upstream storage reset and rollover handling is still basic
- slow-consumer handling is limited to `block` and `disconnect`
- ingest overload handling is limited to payload-size caps and concurrent-client caps
- file mode does not delete old rotated files
- certificate management and deployment policy are still operator-managed

# EXAMPLES

Run the daemon with explicit config:

```text
logjetd --config ./logjetd.conf
```

Inspect generated files:

```text
logjetd inspect ./logs
```

Replay files to a local collector:

```text
logjetd --config ./logjetd.conf replay --path ./logs --name bofh.logjet
```

Bridge from a remote replay listener into a collector:

```text
logjetd --config ./logjetd.conf bridge --source 10.0.0.15:7002
```

# FILES

`/etc/logjet.conf`
: default configuration file

`*.logjet`
: append-only telemetry files written by file mode

`doc/manpage/logjetd.1.md`
: Markdown manpage source

`doc/manpage/logjetd.1`
: generated manpage output

# EXIT STATUS

`0`
: success

non-zero
: error

# SEE ALSO

`pandoc`(1)
