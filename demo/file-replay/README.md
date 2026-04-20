# File Replay Demo

This demo replays recorded `.logjet` files into the OTLP collector mockup
as fast as possible. On first run it generates its own BOFH-flavored
`.logjet` input file locally.

## Build

From the project root:

```bash
make demo
```

That gives you:

- `target/debug/ljd`
- `target/debug/otlp-demo-collector`
- `target/debug/otlp-bofh-logjet-generator`

## Run

From this directory:

```bash
./run-demo.sh
```

The script:

1. starts the OTLP collector mockup
2. creates `./logs/bofh.logjet` with generated OTLP log batches if it does not already exist
3. runs `ljd --config ./logjetd.conf replay --path ./logs --name bofh.logjet`
4. blasts all recorded OTLP batches to the collector

Expected result:

- the collector prints all BOFH log messages

## Notes

- first run seeds `./logs/bofh.logjet`; later runs reuse it
- set `BOFH_RECORD_COUNT` to change how many records are generated on first run
- replay order is `bofh.logjet`, then `bofh-1.logjet`, then `bofh-2.logjet`, and so on
- replay is immediate, no delay
- the collector mockup accepts OTLP/HTTP `POST /v1/logs`
- `collector.url` controls the destination URL
- `collector.timeout-ms` controls the replay socket timeout
