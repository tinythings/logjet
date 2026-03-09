# Replay Handoff Demo

This demo isolates the backlog-to-live handoff inside `ljd serve`.

It proves one replay client can:

- connect after backlog already exists
- receive the retained backlog first
- stay on the same replay connection
- continue receiving new records through direct ingest wakeups

## Build First

From the project root:

```bash
make demo
```

## Run In Two Terminals

Terminal 1, from this directory:

```bash
./run-appliance.sh
```

This starts `ljd`, writes three retained messages before any replay client
connects, and then keeps sending one live message per second.

Terminal 2, from this directory:

```bash
./run-consumer.sh
```

This starts the demo collector and then connects a replay client late through
the internal wire forwarder.

## What You Should See

On the collector side:

- `HANDOFF backlog 001`
- `HANDOFF backlog 002`
- `HANDOFF backlog 003`
- then ongoing `HANDOFF live` records every second

That transition should happen on one replay connection, without reconnecting
between backlog replay and live delivery.
