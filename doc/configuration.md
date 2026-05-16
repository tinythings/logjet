# Configuration

`ljd` reads YAML configuration from:

- `/etc/logjet.conf` by default
- a custom path passed through `-c` or `--config`

## Supported Keys

```yaml
output: buffer          # "buffer" or "file"
buffer.size: 100        # KiB, conflicts with buffer.messages
buffer.messages: 5000   # message count, conflicts with buffer.size
buffer.keep: 1000       # keep first N messages forever
file.path: /foo         # directory, used only when output: file
file.size: 100          # KiB per file segment
file.name: bar.logjet   # base file name
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
ingest.protocol: otlp-http   # "wire", "otlp-http", "otlp-grpc", or "plugin"
ingest.plugin-path: liblj_syslog_ingest.so
ingest.plugin-dir: /usr/lib/logjet/ingestors
ingest.use: syslog
ingest.listen: 127.0.0.1:7001
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

## Key Meanings

### `output`

Selects the local storage mode.

Values:

- `buffer`
  - keep retained records only in memory
- `file`
  - write retained records to append-only `.logjet` files

### `buffer.size`

In-memory rotating tail size limit in KiB.

Use this when you want memory retention bounded by approximate byte size.

Important:

- applies only when `output: buffer`
- conflicts with `buffer.messages`
- does not count as a hard cap for the kept front jar from `buffer.keep`

### `buffer.messages`

In-memory rotating tail size limit by retained message count.

Use this when you want retention bounded by number of messages instead of bytes.

Important:

- applies only when `output: buffer`
- conflicts with `buffer.size`
- does not count as a hard cap for the kept front jar from `buffer.keep`

### `buffer.keep`

Number of first messages to keep forever in memory until the whole buffer is drained.

Think of memory mode as:

```text
[kept front jar][rotating FIFO tail]
```

The first `buffer.keep` messages stay in the front jar and are never evicted by
normal rotation. Only later messages in the FIFO tail rotate out.

Important:

- applies only when `output: buffer`
- if the kept front jar alone exceeds the normal tail limit, those kept messages still remain

### `file.path`

Directory where `.logjet` files are written.

Important:

- applies only when `output: file`
- this is a directory, not a full file name

### `file.size`

Maximum size of one `.logjet` file segment in KiB before rotation.

When the current file exceeds this size, `ljd` opens the next file:

- `name.logjet`
- `name-1.logjet`
- `name-2.logjet`

Important:

- applies only when `output: file`
- old rotated files are kept
- use `ljd segments --path ... --name ...` to inspect one rotated spool
- use `ljd prune --path ... --name ... --keep-files <n>` to trim oldest archived segments by file count
- use `ljd prune --path ... --name ... --keep-bytes <bytes> --dry-run` to preview byte-budget pruning without deleting files

### `file.name`

Base file name for file mode.

Example:

- `app.logjet`
- `app-1.logjet`
- `app-2.logjet`

Important:

- applies only when `output: file`

### `collector.url`

Default bridge and replay destination setting.

Accepted forms:

- one destination string:
  - `http://127.0.0.1:4318/v1/logs`
  - `https://127.0.0.1:4318/v1/logs`
  - `grpc://127.0.0.1:4317`
  - `127.0.0.1:4318`
- a YAML list of destination strings:

```yaml
collector.url:
  - http://127.0.0.1:4318/v1/logs
  - grpc://127.0.0.1:4317
```

If only host and port are given, replay defaults to:

```text
/v1/logs
```

Scheme behaviour:

- `http://`
  - OTLP/HTTP export
- `https://`
  - OTLP/HTTP export over TLS
- `grpc://`
  - OTLP/gRPC export
- `grpcs://`
  - OTLP/gRPC export over TLS
- bare `host:port`
  - treated as OTLP/HTTP with `/v1/logs`

Bridge fan-out behaviour:

- `ljd bridge` sends each OTLP log batch to every configured collector destination
- in `drain` mode, upstream acknowledgement happens only after every configured destination accepts the batch
- if any destination fails, that batch is treated as failed and reconnect or retry logic takes over

Replay behaviour:

- `ljd replay` also uses `collector.url` when `--dest` is omitted
- if `collector.url` is a list, replay sends each batch to every listed destination
- `--dest` still overrides replay with one explicit destination

### `collector.timeout-ms`

Socket timeout in milliseconds used by `ljd replay` when posting stored
OTLP payloads to `collector.url`.

It is also used by `ljd bridge` when posting replayed OTLP payloads to the
configured collector destination set.

### `collector.ca-file`

CA file used by every TLS collector destination in `collector.url`.

Rules:

- applies to `https://...` and `grpcs://...`
- ignored for `http://...`, `grpc://...`, and bare `host:port`
- one config value is shared across all TLS collector destinations in the same `ljd` process
- if different TLS destinations need different CA roots, split them across separate `ljd` bridge or replay processes

### `collector.cert-file`

Optional client certificate used by every TLS collector destination in `collector.url`.

Rules:

- applies to `https://...` and `grpcs://...`
- ignored for `http://...`, `grpc://...`, and bare `host:port`
- must be paired with `collector.key-file`
- one config value is shared across all TLS collector destinations in the same `ljd` process
- if different TLS destinations need different client identities, split them across separate `ljd` bridge or replay processes

### `collector.key-file`

Private key matching `collector.cert-file`.

Rules:

- applies to `https://...` and `grpcs://...`
- ignored for `http://...`, `grpc://...`, and bare `host:port`
- must be paired with `collector.cert-file`

### `collector.server-name`

Override server name used for TLS collector certificate validation.

Rules:

- applies to `https://...` and `grpcs://...`
- ignored for `http://...`, `grpc://...`, and bare `host:port`
- one override is shared across all TLS collector destinations in the same `ljd` process
- use it only when every TLS destination can validate against the same override name
- if different TLS destinations need different server names, split them across separate `ljd` bridge or replay processes

### gRPC bridge TLS examples

Plain TLS with server certificate validation:

```yaml
collector.url: grpcs://collector.example:4317
collector.ca-file: /etc/logjet/collector-ca.pem
collector.server-name: collector.example
```

Mutual TLS with client certificate authentication too:

```yaml
collector.url: grpcs://collector.example:4317
collector.ca-file: /etc/logjet/collector-ca.pem
collector.cert-file: /etc/logjet/collector.pem
collector.key-file: /etc/logjet/collector.key
collector.server-name: collector.example
```

Interpretation:

- plain TLS means the bridge verifies the collector certificate and does not send a client certificate
- mutual TLS means the bridge verifies the collector certificate and also sends its own client certificate
- `collector.cert-file` and `collector.key-file` must be set together or both left unset
- `collector.ca-file` is required for `grpcs://...`

### Mixed TLS fan-out rules

One `collector.url` list may mix plain and TLS destinations, for example:

```yaml
collector.url:
  - http://127.0.0.1:4318/v1/logs
  - grpc://127.0.0.1:4317
  - grpcs://collector.example:4317
```

Behaviour:

- plain destinations ignore `collector.ca-file`, `collector.cert-file`, `collector.key-file`, and `collector.server-name`
- TLS destinations share one collector TLS client configuration inside one `ljd` process
- `https://...` and `grpcs://...` therefore reuse the same CA roots, optional client certificate, optional client key, and optional server-name override
- mixed plain plus TLS fan-out is supported
- mixed TLS fan-out is supported only when every TLS destination can use the same TLS client settings
- if one TLS destination fails handshake or export, that batch is treated as failed for the whole fan-out set
- in bridge `drain` mode, upstream acknowledgement waits until every configured destination accepts the batch

Operational rule:

- if destinations need different trust roots, different client certificates, or different server-name overrides, run separate `ljd` instances

### `ingest.max-batch-bytes`

Maximum accepted payload size in bytes for one ingest record or one OTLP batch.

Use this to stop oversized senders from consuming too much memory or CPU on
weak appliances.

Important:

- default is `1048576`
- applies to `wire`, `otlp-http`, and `otlp-grpc`
- oversized payloads are rejected before they are appended

### `ingest.max-clients`

Maximum number of ingest clients handled at the same time.

Use this to stop a burst of simultaneous senders from overwhelming the daemon.

Important:

- default is `32`
- must be greater than zero
- applies directly to thread-per-client ingest paths and current gRPC concurrency handling
- plain non-TLS `otlp-http` ingest is already serial in the current implementation

### `ingest.max-batches-per-second`

Maximum number of accepted ingest batches per second.

Important:

- default is `0`, which disables rate limiting
- applies to `wire`, `otlp-http`, and `otlp-grpc`
- OTLP batches above the limit can still be accepted when their severity reaches `ingest.priority-severity-at-least`

### `ingest.priority-severity-at-least`

Minimum OTLP log severity that is allowed to bypass overload shedding when
`ingest.max-batches-per-second` is exceeded.

Accepted values:

- `trace`
- `debug`
- `info`
- `warn`
- `error`
- `fatal`

Important:

- default is `error`
- applies to decoded OTLP log batches
- metrics and traces do not carry severity in OTLP, so they are always treated as lowest priority during overload
- wire ingest records do not carry OTLP severity, so they are treated as low priority during overload

### `ingest.overload-report-ms`

How often `ljd` prints aggregated ingest overload counters to stderr while
overload events are happening.

Important:

- default is `5000`
- `0` disables the periodic overload summary
- counters include accepted, priority-bypass, rate-limited, oversize-rejected, and client-cap-rejected batches

### `replay.max-clients`

Maximum number of replay clients handled at the same time.

Use this to stop too many downstream replay or bridge connections from
consuming threads and replay-side resources.

Important:

- default is `32`
- must be greater than zero
- applies to the replay listener used by downstream `keep` and `drain` clients
- extra clients are closed when the limit is already reached

### `replay.client-timeout-ms`

Per-client socket timeout for replay connections in milliseconds.

Use this to stop one stuck or extremely slow replay client from holding a replay
thread forever.

Important:

- default is `10000`
- must be greater than zero
- applies to replay request reads, record writes, flushes, and drain acknowledgements
- timeout currently closes that client connection; other replay clients keep their own threads and cursors

### `backpressure.mode`

Bridge export behaviour when the collector is slower than the bridge.

Values:

- `disconnect`
  - use `collector.timeout-ms` as a socket timeout
  - if the collector is too slow, bridge export fails and reconnect logic takes over
- `block`
  - do not use collector socket timeouts
  - the bridge waits as long as needed for the collector reply
- `drop-newest`
  - keep using collector socket timeouts
  - if the bridge-side export queue is full, the newest record is dropped explicitly

Important:

- default is `disconnect`
- this setting affects bridge export to the collector
- `ljd replay` remains a one-shot bulk operation and does not use this policy

### `backpressure.max-buffered-records`

Maximum number of OTLP log batches the bridge keeps in its local export queue
per bridge connection when `backpressure.enabled: true`.

Important:

- default is `16`
- must be greater than zero
- applies only to `ljd bridge`
- `block` waits for queue space
- `disconnect` fails the bridge connection when the queue is full
- `drop-newest` drops the newest queued record when the queue is full

### `backpressure.enabled`

Enable or disable bridge backpressure policy handling.

Values:

- `false`
  - default
  - bridge uses normal collector socket timeouts
- `true`
  - bridge applies `backpressure.mode`

Important:

- default is `false`
- `backpressure.mode` matters only when `backpressure.enabled: true`
- this switch affects bridge export only

### `upstream.replay`

Source `host:port` for `ljd bridge`.

This should point at another `ljd` replay listener, not an OTLP endpoint.

Example:

- `10.0.0.15:7002`

If this key is omitted, `ljd bridge` requires `--source`.

### `upstream.mode`

Retention mode requested by `ljd bridge` from the upstream replay listener.

Values:

- `keep`
  - replayed records stay on the upstream side after forwarding
- `drain`
  - replayed records are acknowledged and then consumed on the upstream side

Important:

- default is `keep`
- use `drain` when replay should behave like a queue instead of a replayable backlog
- in `drain` mode, the downstream bridge acknowledges each record only after successful export to every configured collector destination
- in file mode, fully consumed closed segments are deleted; the current active segment stays logically empty until rotation or reopen

### `upstream.state-file`

Optional local file used by `ljd bridge` to persist the last successfully
forwarded sequence.

Important:

- default is unset
- when set, bridge loads the saved sequence at start-up
- bridge writes the new sequence after each successful export to every configured collector destination
- this allows restart resume instead of restarting from sequence zero
- the saved state also carries upstream stream identity
- that lets bridge detect upstream restart or storage replacement and reset stale saved sequence state
- the state file lives on the downstream bridge side, not the upstream appliance side

### `upstream.retry-ms`

Reconnect delay in milliseconds for `ljd bridge`.

When the upstream replay connection closes or fails, bridge mode waits this
long before reconnecting.

### `upstream.connect-timeout-ms`

TCP connect timeout in milliseconds for `ljd bridge` when opening the
upstream replay connection.

### `tls.enable`

Enable TLS for the daemon-to-daemon replay transport.

This affects:

- the replay listener exposed by `serve`
- the upstream replay connection used by `bridge`

Use `ingest.*` for OTLP listener TLS and `collector.*` for HTTPS collector
export.

### `tls.ca-file`

PEM file containing CA certificates for replay/bridge TLS validation.

Use cases:

- bridge client verifies the replay listener certificate
- replay listener verifies client certificates when `tls.require-client-cert: true`

### `tls.cert-file`

PEM file containing the local certificate for replay/bridge TLS.

Use cases:

- replay listener presents this certificate when `tls.enable: true`
- bridge presents this certificate when mutual TLS is used

### `tls.key-file`

PEM file containing the private key matching `tls.cert-file`.

### `tls.require-client-cert`

Require client certificates on the replay listener.

When enabled:

- replay listener requires a client certificate
- `tls.ca-file` must be set on the server side

### `tls.server-name`

Override server name used by `ljd bridge` for TLS certificate validation.

Use this when:

- the replay listener is reached by IP address
- but the certificate is issued for a DNS name

### `ingest.protocol`

Selects how `ljd` accepts incoming telemetry.

Values:

- `wire`
  - the current internal framed TCP protocol used by `ljd` replay clients and custom clients
  - this is not OTLP
- `otlp-http`
  - OTLP over HTTP protobuf
  - accepts `POST /v1/logs`, `POST /v1/metrics`, and `POST /v1/traces`
- `otlp-grpc`
  - OTLP over gRPC
  - accepts the standard `LogsService/Export`, `MetricsService/Export`, and `TraceService/Export` RPCs
- `plugin`
  - loads an ingest plugin shared library
  - passive plugins receive bytes from the `ingest.listen` TCP listener
  - active plugins own their input source through `lj_ingest_fetch`

If you want a normal OpenTelemetry producer to send logs directly to `ljd`,
use either `otlp-http` or `otlp-grpc`.

### `ingest.plugin-path`

Shared-library path or search directory used when `ingest.protocol: plugin`.

If the value is an explicit path such as `./plugins/liblj_syslog_ingest.so` or
`/opt/logjet/liblj_syslog_ingest.so`, `ljd` loads that path directly.

If the value is a bare filename such as `liblj_syslog_ingest.so`, `ljd`
searches these roots in order:

1. entries from `LJD_INGEST_PLUGIN_PATH`, split like a normal platform path list
2. `./ingestors`
3. `<ljd executable directory>/ingestors`
4. `<ljd executable directory>/../lib/logjet/ingestors`
5. on Unix, `/usr/lib/logjet/ingestors`
6. on Unix, `/usr/lib/logjet`

Use the `ingestors` subdirectory for packaged ingest plugins. `/usr/lib/logjet`
is also checked for simple system installs.

If `ingest.use` or `ingest.plugin` is set, `ingest.plugin-path` may instead be
a directory. In that mode, `ljd` scans the directory plus the normal search
roots and selects the plugin whose descriptor name matches.

Example:

```yaml
ingest.protocol: plugin
ingest.plugin-path: /opt/plugins
ingest.use: logcat
```

`ingest.plugin-dir` is also accepted as an explicit scan directory:

```yaml
ingest.protocol: plugin
ingest.plugin-dir: /opt/plugins
ingest.plugin: logcat
```

Built-in ingest plugin descriptor names are `syslog`, `logcat`, and `stress`.
`ingest.use` and `ingest.plugin` are aliases; if both are present they must
have the same value.

Use `ljd --plugins` to list the ingest and export plugins visible from the
current environment. Passing `--config` includes configured ingest plugin
locations such as `ingest.plugin-path` and `ingest.plugin-dir` in that listing.

### `ingest.tls-enable`

Enable TLS for OTLP ingest listeners.

Behaviour:

- with `ingest.protocol: otlp-http`, `ljd` accepts HTTPS on `/v1/logs`, `/v1/metrics`, and `/v1/traces`
- with `ingest.protocol: otlp-grpc`, `ljd` accepts gRPC over TLS

### `ingest.ca-file`

CA file used to verify client certificates when `ingest.require-client-cert:
true`.

### `ingest.cert-file`

Server certificate for TLS-enabled OTLP ingest.

### `ingest.key-file`

Private key matching `ingest.cert-file`.

### `ingest.require-client-cert`

Require client certificates on TLS-enabled OTLP ingest listeners.

### `ingest.listen`

Address and port where the ingest listener binds.

Examples:

- `127.0.0.1:7001`
- `0.0.0.0:4318`

The meaning of the listener depends on `ingest.protocol`.

### `replay.listen`

Address and port where the daemon replay listener binds.

This replay listener is for the current internal `wire` protocol, not OTLP.
Clients first send a small replay request containing the last sequence they
already have, then the server streams newer records. After the retained backlog
is sent, the replay listener waits for direct wakeups from ingest instead of
sleeping and polling storage.

## Defaults

If omitted:

- `output: buffer`
- `buffer.size: 100`
- `buffer.messages: unset`
- `buffer.keep: 0`
- `file.path: .`
- `file.size: 100`
- `file.name: bar.logjet`
- `collector.url: http://127.0.0.1:4318/v1/logs`
- `collector.timeout-ms: 10000`
- `collector.ca-file: unset`
- `collector.cert-file: unset`
- `collector.key-file: unset`
- `collector.server-name: unset`
- `backpressure.enabled: false`
- `backpressure.mode: disconnect`
- `backpressure.max-buffered-records: 16`
- `upstream.replay: unset`
- `upstream.mode: keep`
- `upstream.state-file: unset`
- `upstream.retry-ms: 1000`
- `upstream.connect-timeout-ms: 5000`
- `tls.enable: false`
- `tls.ca-file: unset`
- `tls.cert-file: unset`
- `tls.key-file: unset`
- `tls.require-client-cert: false`
- `tls.server-name: unset`
- `ingest.protocol: wire`
- `ingest.plugin-path: unset`
- `ingest.plugin-dir: unset`
- `ingest.use: unset`
- `ingest.listen: 127.0.0.1:7001`
- `ingest.tls-enable: false`
- `ingest.ca-file: unset`
- `ingest.cert-file: unset`
- `ingest.key-file: unset`
- `ingest.require-client-cert: false`
- `ingest.max-batch-bytes: 1048576`
- `ingest.max-clients: 32`
- `ingest.max-batches-per-second: 0`
- `ingest.priority-severity-at-least: error`
- `ingest.overload-report-ms: 5000`
- `replay.listen: 0.0.0.0:7002`
- `replay.max-clients: 32`
- `replay.client-timeout-ms: 10000`

## Notes

- sizes are interpreted as KiB
- `buffer.keep` means: keep the first `N` messages in a permanent front jar, then rotate only the later FIFO tail
- set either `buffer.size` or `buffer.messages`, never both
- `buffer.size` limits the rotating in-memory tail by bytes
- `buffer.messages` limits the rotating in-memory tail by message count
- `collector.url` can be one string or a YAML list of strings
- `collector.url` is used by `ljd replay` when `--dest` is omitted
- `collector.timeout-ms` controls replay and bridge collector socket timeout
- `collector.ca-file`, `collector.cert-file`, `collector.key-file`, and `collector.server-name` apply to `https://...` and `grpcs://...` collector export
- one collector TLS config is shared across all TLS collector destinations in one process
- mixed plain plus TLS fan-out is supported
- different TLS trust roots, client certs, or server-name overrides require separate `ljd` instances
- `backpressure.enabled` enables bridge backpressure policy handling
- `backpressure.mode` configures whether bridge export blocks, disconnects, or drops newest records when the collector is too slow
- `backpressure.max-buffered-records` caps the bridge-side exporter queue per bridge connection
- `upstream.replay` is used by `ljd bridge` when `--source` is omitted
- `upstream.mode: keep` leaves upstream retained records in place after replay
- `upstream.mode: drain` consumes upstream retained records after successful bridge export
- `upstream.state-file` stores the last forwarded sequence and upstream stream identity on the downstream bridge side
- `upstream.retry-ms` controls bridge reconnect delay
- `upstream.connect-timeout-ms` controls bridge source connect timeout
- `ingest.tls-*` controls TLS on OTLP/HTTP and OTLP/gRPC ingest
- `ingest.max-batch-bytes` rejects oversized ingest payloads before they are stored
- `ingest.max-clients` caps concurrent ingest handling
- `ingest.max-batches-per-second` caps accepted ingest batches per second
- `ingest.priority-severity-at-least` lets higher-severity OTLP log batches bypass overload shedding
- `ingest.overload-report-ms` controls operator-visible overload summaries on stderr
- `replay.max-clients` caps concurrent replay clients
- `replay.client-timeout-ms` caps how long one replay client can block on socket I/O
- `tls.*` controls optional TLS on the replay listener and bridge source connection
- `ingest.protocol` supports `wire`, `otlp-http`, `otlp-grpc`, and `plugin`
- `ingest.plugin-path` or `ingest.plugin`/`ingest.use` is required when `ingest.protocol: plugin`
- `file.*` settings are ignored unless `output: file`
- `buffer.*` settings are ignored unless `output: buffer`
- `file.path` is treated as a directory, not a full file path
- file mode always rotates to a new append-only file when `file.size` is exceeded
- in file mode, `upstream.mode: drain` deletes fully consumed closed segments
