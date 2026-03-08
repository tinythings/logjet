# logjet gRPC File Demo

This demo wires the OTLP gRPC emitter into `logjetd`, with `logjetd` storing
raw OTLP protobuf batches into `.logjet` files in this directory.

Emitter emits classic BOFH excuses as log messages over OTLP/gRPC.

## Build First

Build everything in development mode from the project root:

```bash
make demo
```

That gives you:

- `target/debug/logjetd`
- `target/debug/otlp-bofh-grpc-emitter`

## Run

From this directory:

```bash
./run-demo.sh
```

The script:

1. starts `logjetd` with the local config file
2. points it at `./logs` for append-only `.logjet` output
3. starts `otlp-bofh-grpc-emitter`
4. keeps the emitter in the foreground

Generated files appear under `./logs`, for example:

- `bofh.logjet`
- `bofh-1.logjet`

## Inspect Output

While the demo is running or after stopping it:

```bash
../../target/debug/logjetd inspect ./logs
```

## Notes

- ingest is OTLP/gRPC on `127.0.0.1:4317`
- replay listener is configured but not used in this demo
- file rotation is controlled by `file.size` in `logjetd.conf`
- `file.size` is measured in KiB
- old rotated files are kept
- file mode does not use `buffer.keep`
