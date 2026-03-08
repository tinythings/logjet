# Memory Buffer Demo

This demo proves the memory retention model:

```text
[kept messages][rotating tail]
```

The demo keeps the first 3 messages forever and rotates only the later tail.

## Build First

From the project root:

```bash
make demo
```

That gives you:

- `target/debug/logjetd`
- `target/debug/otlp-bofh-emitter`
- `target/debug/otlp-demo-collector`
- `target/debug/otlp-wire-forwarder`

## Run

From this directory:

```bash
./run-demo.sh
```

The script:

1. starts `logjetd` in memory mode with:
   - `buffer.keep: 3`
   - `buffer.messages: 10`
2. emits 10 startup messages:
   - `this message #1 must be kept`
   - ...
   - `this message #10 must be kept`
3. emits 100 BOFH flood messages with no delay
4. starts the colorful OTLP collector mockup
5. connects a wire forwarder to `logjetd` replay
6. forwards exactly 13 messages to the collector

Expected result:

- the collector shows:
  - the first 3 startup messages
  - the last 10 flood messages
- startup messages `#4` through `#10` are gone

## Why 13 Messages

- kept front jar: 3
- rotating tail: 10
- total retained after the late consumer connects: 13

## Notes

- ingest is OTLP/HTTP on `127.0.0.1:4318`
- replay listener is the internal wire protocol on `127.0.0.1:7002`
- collector mockup listens on `127.0.0.1:4320`
- this demo is intentionally deterministic

## Memory Show Variant

If you want a smaller visible run where startup messages `#4` through `#10` are
still present together with a few BOFH messages, run:

```bash
./run-demo-memshow.sh
```

This variant uses:

- `buffer.keep: 3`
- total visible ring of 15 messages
- rotating tail size of 12 messages
- 10 startup messages
- 5 BOFH flood messages

Expected result:

1. you see the first 3 startup messages
2. you see startup messages `#4` through `#10`
3. you see 5 BOFH flood messages

Total retained and forwarded:

- kept front jar: 3
- rotating tail: 12
- total shown: 15

Why the config says `buffer.messages: 12`:

- in `logjetd`, `buffer.messages` applies only to the rotating tail
- the first 3 kept messages are outside that limit
- so `3 kept + 12 tail = 15 total visible messages`
