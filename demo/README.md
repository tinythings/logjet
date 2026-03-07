# BOFH Demo

This directory contains two binaries for a two-terminal OTLP demo over TCP.

- `otlp-bofh-emitter`
  - emits a real OTLP/HTTP protobuf log batch every second
  - prints the exact log content it is sending, without ANSI colors

- `otlp-demo-collector`
  - listens on a TCP socket for OTLP/HTTP `POST /v1/logs`
  - decodes the protobuf payload
  - prints the same log content to stdout with ANSI colors

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

The emitter prints plain output like:

```text
service=bofh-emitter scope=logjet-demo-emitter severity=WARN ts=1700000000000000000
message: BOFH excuse #1: magnetic interference from a mislabeled coffee mug
```

The collector prints the same fields, but colorized.

## Defaults

If you do not pass an address:

- collector binds to `0.0.0.0:4318`
- emitter sends to `127.0.0.1:4318`

## Notes

- the transport is OTLP/HTTP protobuf
- the collector is intentionally tiny and is only for demos and quick local setups
- this is useful when setting up a real collector like Vector would be overkill
