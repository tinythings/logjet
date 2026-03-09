# Multi-Emitter Continuous Demo

This demo shows a live screen full of mixed messages from several emitters at
the same time.

It uses:

- five OTLP/HTTP emitters running continuously
- one `ljd`
- one OTLP collector mockup
- one wire forwarder from the `ljd` replay listener into the collector

Each emitter identifies itself through `service.name`:

- `Alice`
- `Bob`
- `Carol`
- `Dave`
- `Eve`

The collector shows all of them mixed together as one live retained stream.

## Build First

From the project root:

```bash
make demo
```

## Run In Two Terminals

Terminal 1, from this directory:

```bash
./run-emitters.sh
```

This starts:

1. starts `ljd`
2. starts five continuous emitters with different service names

Terminal 2, from this directory:

```bash
./run-consumer.sh
```

This starts:

1. the OTLP collector
2. the wire forwarder to the collector

There is also a single-terminal convenience script:

```bash
./run-demo.sh
```

Expected result:

- the collector screen fills with interleaved BOFH messages
- each line shows a different `service=` value such as `Alice` or `Bob`
- all five emitters keep sending until you stop the script

Stop it with `Ctrl+C`.

This demonstrates:

- several emitters can connect to one `ljd` at the same time
- `ljd` merges their records into one retained stream
- one downstream consumer can watch that mixed stream live
