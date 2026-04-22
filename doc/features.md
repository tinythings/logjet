# ljd Features

This file tracks the current feature set and practical use cases of `ljd`.
It is meant to evolve as the daemon grows.

## Current Features

### 1. OTLP log ingest

`ljd` can accept real OTLP/HTTP protobuf log export requests on:

```text
POST /v1/logs
```

It can also accept OTLP/gRPC log export requests on the standard
`LogsService/Export` endpoint when `ingest.protocol: otlp-grpc` is configured.

Current behaviour:

- accepts OTLP log batches over HTTP and gRPC
- OTLP/HTTP ingest can also run over HTTPS
- OTLP/gRPC ingest can also run over TLS
- rejects oversized batches through `ingest.max-batch-bytes`
- can cap concurrent ingest handling through `ingest.max-clients`
- can rate-limit accepted ingest batches through `ingest.max-batches-per-second`
- can keep higher-severity OTLP log batches during overload through `ingest.priority-severity-at-least`
- emits overload counters on stderr through `ingest.overload-report-ms`
- validates that the request decodes as `ExportLogsServiceRequest`
- stores the raw OTLP protobuf bytes
- assigns a local sequence number for internal replay ordering

### 2. Append-only `.logjet` file output

In file mode, `ljd` writes raw OTLP protobuf batches into `.logjet` files
using the `logjet` block format.

Current behaviour:

- append-only writes
- file rotation when `file.size` is exceeded
- after restart, file-mode sequence numbering resumes from the highest stored sequence
- rotated files are kept
- naming style:
  - `name.logjet`
  - `name-1.logjet`
  - `name-2.logjet`
- file-mode operational tooling exists through:
  - `ljd segments --path ... --name ...`
  - `ljd prune --path ... --name ... --keep-files ...`
  - `ljd prune --path ... --name ... --keep-bytes ...`

### 3. In-memory ring buffer mode

In buffer mode, `ljd` can hold retained records in RAM.

Current behaviour:

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

`ljd` exposes a replay socket for downstream consumers.

Current behaviour:

- clients send a small replay request with `from_seq`
- the server first sends a replay hello carrying stream identity and current bounds
- clients can request `keep` or `drain`
- each client keeps its own replay cursor
- replays retained data in sequence order
- hands off from retained backlog to live records through direct ingest wakeups without returning to storage polling
- supports multiple clients in a basic way
- can cap concurrent replay clients through `replay.max-clients`
- can cap blocked replay socket I/O through `replay.client-timeout-ms`

Current limitation:

- replay uses a custom internal wire protocol, not OTLP egress yet

### 5. Continuous bridge mode

`ljd` can run as a downstream bridge process with:

```text
ljd bridge [--source <host:port>]
```

Current behaviour:

- connects to another `ljd` replay listener
- requests replay starting after the last sequence already forwarded
- can keep upstream records or drain them, depending on `upstream.mode`
- stays attached and forwards new log records live
- posts raw stored OTLP protobuf payloads to every destination configured in `collector.url`
- supports OTLP/HTTP export, HTTPS export, plain OTLP/gRPC export, and gRPC export over TLS or mutual TLS
- acknowledges records in `drain` mode only after successful export to every configured destination
- can fan one upstream stream out to multiple downstream collectors from one `ljd` instance
- mixed plain plus TLS fan-out works when every TLS destination can share one collector TLS client config
- if TLS destinations need different CA roots, client certs, or server-name overrides, they must be split across separate `ljd` instances
- reconnects after disconnect and resumes from the last forwarded sequence
- can persist that sequence in `upstream.state-file`
- detects upstream restart or storage replacement through replay stream identity
- resets stale saved sequence state instead of waiting forever above a restarted upstream
- bridge export can block or disconnect when the collector is too slow, when `backpressure.enabled: true`
- bridge export can also drop newest records explicitly when `backpressure.mode: drop-newest`
- bridge export queue depth can be capped through `backpressure.max-buffered-records`

This is the current path for:

```text
OA -> ljd <- network <- ljd -> OTel Collector
```

### 6. Optional TLS on replay and bridge transport

The replay listener and bridge client can run over TLS.

Current behaviour:

- TLS is optional and disabled by default
- the replay listener can present a server certificate
- the bridge client can verify the replay listener with `tls.ca-file`
- mutual TLS is supported with client certificates
- replay framing and sequence resume work the same way inside the TLS session

Current limitation:

- TLS config is currently split between replay/bridge (`tls.*`) and OTLP ingest (`ingest.*`)
- collector export uses HTTPS when a `collector.url` entry starts with `https://`
- collector export uses plain OTLP/gRPC when a `collector.url` entry starts with `grpc://`

### 7. One-shot file replay to OTLP collectors

`ljd` can replay stored `.logjet` files directly into OTLP collectors with:

```text
ljd replay --path <dir> --name <base.logjet> [--dest <url-or-host:port>]
```

Current behaviour:

- scans for `name.logjet`, `name-1.logjet`, `name-2.logjet`, and so on
- replays them in that order
- reads stored `logs` records
- posts the raw OTLP protobuf payloads to every configured replay destination
- supports `http://`, `https://`, and `grpc://` collector URLs
- sends as fast as the destination socket allows, with no artificial delay
- if `--dest` is omitted, replay uses `collector.url` from config
- `--dest` still overrides replay with one explicit destination

### 8. YAML configuration

`ljd` reads configuration from:

- `/etc/logjet.conf` by default
- a file passed with `-c` or `--config`

Current config areas:

- output mode
- in-memory buffer sizing
- file rotation sizing
- ingest protocol
- ingest and replay bind addresses
- replay client cap
- collector URL and timeout
- collector TLS trust/client-cert settings
- bridge backpressure enable switch
- bridge backpressure mode
- ingest payload-size guardrails
- ingest concurrent-client guardrails
- upstream replay source and retry behaviour
- upstream keep-or-drain behaviour
- upstream persisted resume state
- replay/bridge TLS settings
- OTLP ingest TLS settings

### 9. Inspection tooling

`ljd` can inspect stored `.logjet` files or directories and print metadata
about retained records.

### 10. File-mode operational tooling

`ljd` can report and prune rotated file segments explicitly.

Current behaviour:

- `ljd segments --path <dir> --name <base.logjet>` prints ordered segment metadata
- output includes:
  - segment id
  - file path
  - file size
  - record count
  - first and last sequence in that segment
- `ljd prune --path <dir> --name <base.logjet> --keep-files <n>` removes oldest rotated segments and keeps the newest `n` segment files
- `ljd prune --path <dir> --name <base.logjet> --keep-bytes <bytes>` removes oldest rotated segments until the newest retained set fits within the byte budget
- `--dry-run` shows what would be removed without deleting anything
- the newest active segment is always retained

## Current Use Cases

### 1. Local OTLP capture to files

Use case:

- an emitter sends OTLP logs locally
- `ljd` stores them in `.logjet` files
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

- constrained and mission-critical environments need cheap, predictable runtime behaviour
- storage and networking are unreliable

### 4. Demo and lab setups without a full collector stack

Use case:

- use the OTLP demo emitter
- send logs into `ljd`
- store them or inspect them locally

Useful when:

- a full OTel Collector deployment would be overkill
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

### 6. File archive housekeeping outside the daemon

Use case:

- file mode keeps rotated segments on disk
- an operator wants to inspect segment growth and prune old archives deliberately
- file retention should be managed as an explicit operational step rather than hidden daemon behaviour

Useful when:

- archived files must be trimmed before shipping or collection
- retention differs between deployments
- operators need a safe dry-run before removing old segments

### 7. Continuous remote drain into a collector

Use case:

- one `ljd` instance runs next to `OA`
- a second `ljd` instance connects to the first over the network
- the second instance forwards retained backlog and live OTLP logs into an OTel Collector

Useful when:

- the appliance cannot push directly to the final collector
- an external side must initiate the connection
- you need a lightweight relay instead of deploying a full collector locally

## Current Non-Features

These are not implemented yet:

- advanced restart behaviour when upstream storage is reset or replaced
- advanced slow-consumer handling
- ingest rate limiting or priority-aware shedding
- disk-budget retention management for rotated files
- production-grade service lifecycle handling
