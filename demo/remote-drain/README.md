# Remote Drain Demo

This demo shows the intended network shape:

```text
OA -> logjetd <- network <- logjetd -> OTel Collector
```

In this demo:

- the appliance-side `logjetd` accepts OTLP/HTTP logs locally
- the appliance-side emitter stands in for `OA`
- the remote-side `logjetd` runs in `bridge` mode
- the remote-side collector mockup stands in for an OTel Collector
- the appliance-side daemon uses memory retention with a permanent kept prefix

The important point is:

- the remote side initiates the connection to the appliance replay listener
- the appliance side does not need to connect outward

## Build First

From the project root:

```bash
make demo
```

That gives you:

- `target/debug/logjetd`
- `target/debug/otlp-bofh-emitter`
- `target/debug/otlp-demo-collector`

## Terminal 1: Appliance Side

From this directory:

```bash
./run-appliance.sh
```

This starts:

1. appliance-side `logjetd` with local OTLP/HTTP ingest
2. memory retention with:
   - `buffer.keep: 3`
   - `buffer.messages: 5`
3. 8 manual startup messages:
   - `BOOT MESSAGE #1`
   - ...
   - `BOOT MESSAGE #8`
4. continuous BOFH traffic pointing at the appliance-side daemon

The appliance-side replay listener binds to:

```text
127.0.0.1:7002
```

## Terminal 2: Remote Side

From this directory:

```bash
./run-remote.sh
```

This starts:

1. the OTLP collector mockup on `127.0.0.1:4320`
2. remote-side `logjetd bridge`
3. a connection from the remote side into the appliance replay listener

Expected result:

- `BOOT MESSAGE #1`, `BOOT MESSAGE #2`, and `BOOT MESSAGE #3` always appear
- the other boot messages may or may not appear, depending on when the remote side starts
- backlog already retained on the appliance side is drained first
- then new BOFH messages continue to appear live in the collector

Why this is timing-dependent:

- the first 3 boot messages live in the permanent kept front jar
- boot messages `#4` through `#8` live in a rotating tail of only 5 messages
- once continuous BOFH traffic starts, those tail messages can be pushed out
- if you start the remote side quickly, you may still catch some or all of them

## Notes

- appliance storage mode here is in-memory buffer mode
- appliance replay listener still uses the internal wire protocol
- remote `bridge` forwards OTLP logs to `collector.url`
- this demo models the real direction of connection initiation
