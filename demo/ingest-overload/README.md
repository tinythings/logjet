# Ingest Overload Demo

This demo shows the ingest overload policy rather than bridge-side
backpressure.

It uses:

- a small ingest rate limit: `ingest.max-batches-per-second: 3`
- severity-aware shedding: `ingest.priority-severity-at-least: error`
- overload summaries on stderr: `ingest.overload-report-ms: 200`

The intended effect is:

- the first few `WARN` batches fit under the rate limit
- later `WARN` batches are rejected while the overload window is full
- `ERROR` batches still get through during that same overload window
- `logjetd` prints overload counters for operator visibility

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

This starts `logjetd`, then sends:

1. a fast `WARN` burst from `service=overload-warn`
2. a fast `ERROR` burst from `service=overload-error`
3. another fast `WARN` burst

Expected appliance-side output:

- many `WARN` sends fail with `HTTP/1.1 429 Too Many Requests`
- `ERROR` sends succeed even though the daemon is already overloaded
- `logjetd` prints lines such as:

```text
logjetd ingest overload stats accepted=... priority-bypass=... rate-limited=...
```

## Terminal 2: Consumer Side

From this directory:

```bash
./run-consumer.sh
```

This starts:

1. the OTel Collector mock
2. the wire forwarder from `logjetd` replay into the collector

Expected collector output:

- only a few early `WARN` records are present
- `ERROR` records are present even though they arrived during overload
- many `WARN` records are absent because the ingest policy shed them

## Point of the Demo

This demonstrates all three pieces of the ingest overload policy:

- ingest rate limiting
- operator-visible overload counters
- priority-aware shedding based on OTLP log severity
