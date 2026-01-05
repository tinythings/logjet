# File Replay Demo

This demo replays previously recorded `.logjet` files into the OTLP collector mockup
as fast as possible. This demo depends on the first demo.

## Build

From the project root:

```bash
make devel
make demo
```

That gives you:

- `target/debug/logjetd`
- `target/debug/otlp-demo-collector`


## Setup

Run the first demo in [`../logjet-file`](../logjet-file) so it creates a
`logs/` directory with files such as:

- `bofh.logjet`
- `bofh-1.logjet`
- `bofh-2.logjet`

Terminate messages recording manually (Ctrl+C). Then move or copy that `logs/`
directory into this current directory:


```bash
mv ../logjet-file/logs ./logs
```

## Run

From this directory:

```bash
./run-demo.sh
```

The script:

1. starts the OTLP collector mockup
2. runs `logjetd --config ./logjetd.conf replay --path ./logs --name bofh.logjet`
3. blasts all recorded OTLP batches to the collector

Expected result:

- the collector prints all BOFH log messages

## Notes

- replay order is `bofh.logjet`, then `bofh-1.logjet`, then `bofh-2.logjet`, and so on
- replay is immediate, no delay
- the collector mockup accepts OTLP/HTTP `POST /v1/logs`
- `collector.url` controls the destination URL
- `collector.timeout-ms` controls the replay socket timeout
