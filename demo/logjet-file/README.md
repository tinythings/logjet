# logjet File Demo

This demo wires the OTLP emitter into `logjetd`, with `logjetd` storing
raw OTLP protobuf batches into `.logjet` files in this directory.

Emitter emits classic BOFH excuses as log messages. :-)

## Build First

Build everything in development mode from the project root:

```bash
make demo
```

That gives you:

- `target/debug/logjetd`
- `target/debug/otlp-bofh-emitter`

## Run

From this directory:

```bash
./run-demo.sh
```

The script:

1. starts `logjetd` with the local config file
2. points it at this directory for append-only `.logjet` output
3. starts `otlp-bofh-emitter`
4. keeps the emitter in the foreground

Generated files appear here, for example:

- `bofh.logjet`
- `bofh-1.logjet`

etc.

## Inspect Output

While the demo is running or after stopping it:

```bash
../../target/debug/logjetd inspect .
```

## Notes

- ingest is OTLP/HTTP on `127.0.0.1:4318`
- replay listener is configured but not used in this demo
- file rotation is controlled by `file.size` in `logjetd.conf`
- `file.size` is measured in KiB
- old rotated files are kept
- file mode does not use `buffer.keep`
