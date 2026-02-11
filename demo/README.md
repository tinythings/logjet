# BOFH Demo

This directory contains two binaries for a two-terminal OTLP demo over TCP.

- `otlp-bofh-emitter`
  - emits a real OTLP/HTTP protobuf log batch every second
  - prints the exact log content it is sending, without ANSI colours

- `otlp-bofh-grpc-emitter`
  - emits a real OTLP/gRPC log batch every second
  - prints the exact log content it is sending, without ANSI colours

- `otlp-demo-collector`
  - listens on a TCP socket for OTLP/HTTP `POST /v1/logs`
  - decodes the protobuf payload
  - prints the same log content to stdout with ANSI colours

It also contains scenario demos under subdirectories:

- [`logjet-file`](./logjet-file)
  - OTLP/HTTP emitter into file-backed `logjetd`
- [`logjet-grpc-file`](./logjet-grpc-file)
  - OTLP/gRPC emitter into file-backed `logjetd`
- [`memory-buffer`](./memory-buffer)
  - kept-front-jar plus rotating-tail memory retention
- [`file-replay`](./file-replay)
  - replay stored `.logjet` files into a collector
- [`remote-drain`](./remote-drain)
  - appliance-side `logjetd` drained by a remote-side `logjetd bridge`
- [`remote-drain-tls`](./remote-drain-tls)
  - same remote-drain topology, but with TLS and mutual TLS on the replay link
- [`secure-pipeline`](./secure-pipeline)
  - HTTPS OTLP ingest into `logjetd`, then HTTPS collector export on replay

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

The collector prints the same fields, but colourised.

## Defaults

If you do not pass an address:

- collector binds to `0.0.0.0:4318`
- emitter sends to `127.0.0.1:4318`

## Notes

- the transport is OTLP/HTTP protobuf
- the gRPC emitter uses OTLP/gRPC logs export
- the collector is intentionally tiny and is only for demos and quick local setups
- this is useful when setting up a real collector like Vector would be overkill
