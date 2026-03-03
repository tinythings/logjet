# Backpressure Demo

This demo shows what happens when the collector is slower than the bridge.

It demonstrates two policies:

- `backpressure.mode: disconnect`
- `backpressure.mode: block`
- `backpressure.mode: drop-newest`

The slow collector is the key part. It accepts an OTLP request, prints it, then
waits before returning `200 OK`.

That means:

- the appliance keeps producing faster than the collector accepts
- the bridge cannot pretend delivery has happened already
- the configured backpressure policy becomes visible

## Build First

From the project root:

```bash
make demo
```

## Terminal 1: Appliance Side

From this directory:

```bash
./run-appliance.sh
```

This starts:

1. appliance-side `logjetd`
2. a fast continuous emitter

The emitter sends every 200 ms.

## Terminal 2: Consumer Side

You can choose one of three consumer scripts.

### Disconnect policy

```bash
./run-consumer-disconnect.sh
```

This uses:

```yaml
backpressure.enabled: true
backpressure.mode: disconnect
backpressure.max-buffered-records: 3
collector.timeout-ms: 1000
```

The collector waits 2000 ms before replying.

Expected result:

- the collector is slower than the timeout
- the bridge can only queue three records locally
- bridge export times out
- the bridge disconnects and retries
- backlog stays on the appliance side

### Block policy

```bash
./run-consumer-block.sh
```

This uses:

```yaml
backpressure.enabled: true
backpressure.mode: block
backpressure.max-buffered-records: 3
collector.timeout-ms: 1000
```

The collector still waits 2000 ms before replying, but the bridge does not use
the timeout as a disconnect trigger.

Expected result:

- the collector prints one message every two seconds
- the bridge waits for each slow reply
- the bridge can queue only three records before it blocks on the exporter queue
- backlog accumulates upstream
- delivery pace is controlled by the collector pace

### Drop-newest policy

```bash
./run-consumer-drop-newest.sh
```

This uses:

```yaml
backpressure.enabled: true
backpressure.mode: drop-newest
backpressure.max-buffered-records: 3
collector.timeout-ms: 1000
```

The collector still waits 2000 ms before replying, but the bridge keeps only a
tiny local queue and explicitly sheds overload once that queue is full.

Expected result:

- the collector still prints slowly
- the bridge keeps forwarding whatever fits into the three-record queue
- newer records are dropped once the queue is full
- terminal output from `logjetd` shows dropped sequence numbers

## Point of the Demo

This shows that:

- successful collector replies control forward progress
- slow downstreams create backpressure
- `disconnect` favours fail-fast retry behaviour
- `block` favours waiting instead of timing out
- `drop-newest` favours keeping the bridge live while explicitly shedding overload
