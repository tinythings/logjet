# Multi-Emitter Demo

This demo shows that several emitters can send OTLP logs into one `ljd`,
and one downstream consumer can receive the merged retained stream later.

The demo uses:

- five separate OTLP/HTTP emitters
- one `ljd` in memory-buffer mode
- one OTLP collector mockup
- one wire forwarder from the `ljd` replay listener into the collector

Each emitter sends one identifying message:

- `I am emitter Alice`
- `I am emitter Bob`
- `I am emitter Carol`
- `I am emitter Dave`
- `I am emitter Eve`

The service name also identifies the emitter, so the collector output shows
which emitter produced each message.

## Build First

From the project root:

```bash
make demo
```

That gives you:

- `target/debug/ljd`
- `target/debug/otlp-bofh-emitter`
- `target/debug/otlp-demo-collector`
- `target/debug/otlp-wire-forwarder`

## Run

From this directory:

```bash
./run-demo.sh
```

The script:

1. starts `ljd`
2. sends five one-shot OTLP messages from five separate emitters
3. starts the collector
4. connects the wire forwarder to the replay listener
5. forwards the retained records into the collector

Expected result:

- the collector prints five records
- each record says `I am emitter <name>`
- each record has a distinct `service=` value matching the emitter name

This demonstrates:

- many emitters can connect to one `ljd`
- `ljd` stores one merged retained stream
- one downstream consumer can receive that merged stream later
