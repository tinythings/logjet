# Ingest Guardrails Demo

This demo shows the two ingest guardrails that protect `ljd` on weak
appliances:

- `ingest.max-batch-bytes`
- `ingest.max-clients`

The point is simple:

- oversized senders should be rejected before they are stored
- a burst of simultaneous clients should not create unlimited concurrent work

## Build First

From the project root:

```bash
make demo
```

## Oversized Batch Rejection

Run:

```bash
./run-oversize.sh
```

This uses [http-limit.conf](./http-limit.conf):

```yaml
ingest.protocol: otlp-http
ingest.max-batch-bytes: 300
```

What happens:

1. `ljd` starts with a tiny OTLP batch limit.
2. A small OTLP batch is sent and accepted.
3. A large OTLP batch is sent and rejected.

Expected result:

- the small batch prints `sent OTLP log batch`
- the oversized batch prints `payload too large`

## Concurrent Client Cap

Run:

```bash
./run-max-clients.sh
```

This uses [wire-limit.conf](./wire-limit.conf):

```yaml
ingest.protocol: wire
ingest.max-clients: 1
```

What happens:

1. `FIRST` connects and keeps the only ingest slot open.
2. `SECOND` tries to connect while `FIRST` is still present.
3. `SECOND` is dropped.
4. After `FIRST` exits, `THIRD` connects and succeeds.

Expected result:

- `FIRST` says its second record was sent and the connection stayed open
- `SECOND` may still report that its first record was sent
- `SECOND` then reports that the connection was closed before the second record
- `THIRD` succeeds once the slot is free again

## Point of the Demo

This shows that `ljd` can now reject bad overload patterns early:

- too large
- too many at once

The concurrent-client part uses plain TCP. That means the refused client can
still complete `connect()` and may even get its first write into the local
socket buffer before `ljd` closes the connection. The important signal is
that the connection does not stay alive for the second record.

That is only the first overload-control layer. It does not yet rate-limit,
sample, or prioritise traffic.
