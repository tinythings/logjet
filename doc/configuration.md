# Configuration

`logjetd` reads YAML configuration from:

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
ingest.protocol: otlp-http   # "wire", "otlp-http", or "otlp-grpc"
ingest.listen: 127.0.0.1:7001
replay.listen: 0.0.0.0:7002
replay.poll_ms: 250
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

When the current file exceeds this size, `logjetd` opens the next file:

- `name.logjet`
- `name-1.logjet`
- `name-2.logjet`

Important:

- applies only when `output: file`
- old rotated files are kept

### `file.name`

Base file name for file mode.

Example:

- `bofh.logjet`
- `bofh-1.logjet`
- `bofh-2.logjet`

Important:

- applies only when `output: file`

### `collector.url`

Default destination used by `logjetd replay` when `--dest` is not provided.

Accepted forms:

- full URL:
  - `http://127.0.0.1:4318/v1/logs`
- host and port only:
  - `127.0.0.1:4318`

If only host and port are given, replay defaults to:

```text
/v1/logs
```

Current limitation:

- `https://` is not supported yet

### `collector.timeout-ms`

Socket timeout in milliseconds used by `logjetd replay` when posting stored
OTLP payloads to `collector.url`.

### `ingest.protocol`

Selects how `logjetd` accepts incoming telemetry.

Values:

- `wire`
  - the current internal framed TCP protocol used by `logjetd` replay clients and custom clients
  - this is not OTLP
- `otlp-http`
  - OTLP over HTTP protobuf
  - accepts `POST /v1/logs`
- `otlp-grpc`
  - OTLP over gRPC
  - accepts the standard logs `Export` RPC

If you want a normal OpenTelemetry producer to send logs directly to `logjetd`,
use either `otlp-http` or `otlp-grpc`.

### `ingest.listen`

Address and port where the ingest listener binds.

Examples:

- `127.0.0.1:7001`
- `0.0.0.0:4318`

The meaning of the listener depends on `ingest.protocol`.

### `replay.listen`

Address and port where the daemon replay listener binds.

This replay listener is for the current internal `wire` protocol, not OTLP.

### `replay.poll_ms`

Polling interval in milliseconds used by the replay listener while waiting for
new records to appear in storage.

Smaller values:

- lower replay latency
- higher wakeup overhead

Larger values:

- lower CPU wakeup overhead
- higher replay latency

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
- `ingest.protocol: wire`
- `ingest.listen: 127.0.0.1:7001`
- `replay.listen: 0.0.0.0:7002`
- `replay.poll_ms: 250`

## Notes

- sizes are interpreted as KiB
- `buffer.keep` means: keep the first `N` messages in a permanent front jar, then rotate only the later FIFO tail
- set either `buffer.size` or `buffer.messages`, never both
- `buffer.size` limits the rotating in-memory tail by bytes
- `buffer.messages` limits the rotating in-memory tail by message count
- `collector.url` is used by `logjetd replay` when `--dest` is omitted
- `collector.timeout-ms` controls replay HTTP socket timeout
- `ingest.protocol` supports `wire`, `otlp-http`, and `otlp-grpc`
- `file.*` settings are ignored unless `output: file`
- `buffer.*` settings are ignored unless `output: buffer`
- `file.path` is treated as a directory, not a full file path
- file mode always keeps everything and only rotates to a new append-only file when `file.size` is exceeded
