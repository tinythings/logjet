# Visual Logtail Demo

This demo starts a tiny emitter that appends one random OTLP log record to a `.logjet`
file every half second, then opens `ljx view --tail` on that file.

## Build First

From the project root:

```bash
cargo build -p ljx -p otlp-demo --bins
```

## Run

From this directory:

```bash
./run-demo.sh
```

## What It Does

1. creates `./logs/visual-logtail.logjet`
2. starts `visual-logtail-emitter` in the background
3. appends one fresh log record every 500 ms
4. opens `ljx view --tail ./logs/visual-logtail.logjet`

The viewer starts in tail mode automatically, so new records should keep landing at
the bottom until you press a key to stop tailing.
