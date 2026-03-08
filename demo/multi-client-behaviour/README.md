# Multi-Client Behaviour Demo

This demo shows the current multi-client replay behaviour:

- each replay client has its own replay cursor
- one stalled replay client does not stop another replay client from receiving records
- `replay.client-timeout-ms` disconnects the stalled client instead of letting it sit forever

## Build First

From the project root:

```bash
make demo
```

## Terminal 1: Appliance

From this directory:

```bash
./run-appliance.sh
```

This starts:

1. appliance-side `logjetd`
2. a steady message stream

The replay listener is configured with:

```yaml
replay.max-clients: 4
replay.client-timeout-ms: 3000
```

## Terminal 2: Normal Consumer

From this directory:

```bash
./run-normal-consumer.sh
```

This starts:

1. the collector
2. a normal replay client that forwards records into the collector

You should immediately see `MULTI 001`, `MULTI 002`, and so on.

## Terminal 3: Stalled Client

From this directory:

```bash
./run-stall-client.sh
```

This client:

1. connects to the replay listener
2. asks for `drain` mode
3. receives one record
4. stops acknowledging it

Because the replay listener uses `replay.client-timeout-ms: 3000`, that stalled
client is closed after a few seconds.

## What You Should See

While the stalled client is running:

- Terminal 2 keeps printing new `MULTI` messages
- Terminal 3 reports that it received one record and then stalled
- appliance-side `logjetd` eventually logs a replay client error caused by timeout

That is the point:

- one replay client can misbehave
- another replay client still has its own thread and its own cursor
- the stuck client does not hold the replay connection open forever

## What This Does Not Prove

This does not prove complete production-grade client isolation.

It only demonstrates the current first layer:

- independent replay cursors
- replay client cap
- replay client timeout
