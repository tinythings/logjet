# BOFH Demo

This directory contains two binaries for a two-terminal OTLP demo over TCP.

- `otlp-bofh-emitter`
  - emits a real OTLP/HTTP protobuf log batch every second
  - prints the exact log content it is sending in plain text

- `otlp-bofh-grpc-emitter`
  - emits a real OTLP/gRPC log batch every second
  - prints the exact log content it is sending in plain text

- `otlp-demo-collector`
  - listens on a TCP socket for OTLP/HTTP `POST /v1/logs`
  - decodes the protobuf payload
  - prints the same log content to stdout with terminal styling

It also contains scenario demos under subdirectories:

- [`logjet-file`](./logjet-file)
  - OTLP/HTTP emitter into file-backed `ljd`
- [`logjet-grpc-file`](./logjet-grpc-file)
  - OTLP/gRPC emitter into file-backed `ljd`
- [`kill-bill`](./kill-bill)
  - cut a `.logjet` file down to its middle third and recover later good blocks
- [`memory-buffer`](./memory-buffer)
  - kept-front-jar plus rotating-tail memory retention
- [`drain-once`](./drain-once)
  - preserved startup messages are consumed on the first drain and do not appear on the second
- [`multi-emitter`](./multi-emitter)
  - five emitters into one `ljd`, then late replay into one collector
- [`multi-emitter-continuous`](./multi-emitter-continuous)
  - five emitters running continuously into one `ljd` and one live collector
- [`multi-client-behaviour`](./multi-client-behaviour)
  - one replay client stalls while another keeps flowing
- [`replay-handoff`](./replay-handoff)
  - a late replay client drains retained backlog and then continues live on the same connection
- [`cpp-shared-lib`](./cpp-shared-lib)
  - a C++ process loads `liblogjet.so`, sends OTLP logs into `ljd`, and opens the result in `ljx view`
- [`file-replay`](./file-replay)
  - replay stored `.logjet` files into a collector
- [`file-tooling`](./file-tooling)
  - inspect rotated file segments and prune archived files deliberately
- [`parquet-export`](./parquet-export)
  - export an existing `.logjet` sample to Parquet through the external exporter plugin
- [`tui-view`](./tui-view)
  - generate 1000 randomized log entries and open `ljx view` on the result
- [`bridge-resume`](./bridge-resume)
  - consumer restart resumes from persisted sequence state without replaying from zero
- [`upstream-reset-resume`](./upstream-reset-resume)
  - consumer bridge detects upstream reset and resumes a fresh stream instead of getting stuck
- [`backpressure`](./backpressure)
  - slow collector demo showing `block`, `disconnect`, and `drop-newest`
- [`ingest-guardrails`](./ingest-guardrails)
  - oversized-batch rejection and concurrent ingest-client cap
- [`ingest-overload`](./ingest-overload)
  - rate-limited ingest with operator-visible counters and severity-aware shedding
- [`remote-drain`](./remote-drain)
  - appliance-side `ljd` drained by a remote-side `ljd bridge`
- [`remote-drain-tls`](./remote-drain-tls)
  - same remote-drain topology, but with TLS and mutual TLS on the replay link
- [`secure-pipeline`](./secure-pipeline)
  - HTTPS OTLP ingest into `ljd`, then HTTPS collector export on replay

## Enjoy It

Open two terminals in the project root.

Terminal 1: start the collector

```bash
cargo run -p otlp-demo --bin otlp-demo-collector -- 127.0.0.1:4318
```

Terminal 2: start the emitter

```bash
cargo run -p otlp-demo --bin otlp-bofh-emitter -- 127.0.0.1:4318
```

Or use the gRPC emitter against an OTLP/gRPC logs endpoint:

```bash
cargo run -p otlp-demo --bin otlp-bofh-grpc-emitter -- 127.0.0.1:4317
```

The emitter prints plain output like:

```text
service=bofh-emitter scope=logjet-demo-emitter severity=WARN ts=1700000000000000000
message: BOFH excuse #1: magnetic interference from a mislabeled coffee mug
```

The collector prints the same fields with terminal styling.

## Defaults

If you do not pass an address:

- collector binds to `0.0.0.0:4318`
- emitter sends to `127.0.0.1:4318`

## Notes

- the transport is OTLP/HTTP protobuf
- the gRPC emitter uses OTLP/gRPC logs export
- the collector is intentionally tiny and is only for demos and quick local setups
- this is useful when setting up a real OTel Collector would be overkill
