# C-API Benchmark (Evidence)

Decomposes the per-record cost of writing OTEL records through the `liblogjet`
C API (`log_record()` / `lj_logger_log`): the per-connection path is slow, and
this demo shows where the time goes.

Reference: [`demo/cpp-shared-lib`](../cpp-shared-lib)

## What it measures

The driver prints one table per transport (OTLP/gRPC, and OTLP/HTTP when an HTTP
endpoint is given). Each table times these phases with real numbers (mean, p50,
p95, p99, min, max) plus an `index` column — the improvement factor
`per-connection mean / row mean` (baseline = that transport's per-connection; the
`logjet file` row's index is a no-network reference floor):

1. **logjet file (`LogjetWriter::push`)** — appending one record straight to a
   `.logjet` file. No network. Isolates the storage/format layer (gRPC table only).
2. **backend (per-connection)** — one `lj_logger_log()` send to `ljd`. The slow
   path: a fresh connection per record.
3. **backend (reuse)** — one `lj_logger_log_reuse()` send, reusing a persistent
   gRPC channel / HTTP keep-alive connection (Tickets 1, 4).
4. **backend (batch=N)** — `lj_logger_log_batch()` sending N records in one request
   over the reused connection (Tickets 2, 4). Reported per-record amortized.
5. **backend (async enqueue)** — `lj_logger_log_async()` hands the send to a
   background runtime and returns immediately (Tickets 3, 4). The row is the
   caller-thread enqueue cost; the demo then calls `lj_logger_flush()` and prints
   the async error/dropped counters.

This makes the finding explicit: the file write is cheap (µs); the per-connection
backend send is the expensive part; connection reuse helps, batching brings the
per-record cost down toward the file-write cost, and async removes the network
round-trip from the caller thread entirely. HTTP reuse uses a keep-alive
connection pool (HTTP/1.1 can't multiplex, so concurrency uses several pooled
connections, bounded by backpressure).

## Columns (in plain terms)

Each number is measured over the messages sent in that row. Times auto-scale:
`ns` < `us` < `ms` < `s` (smaller is faster).

- **calls** — how many log messages this row sent.
- **total** — all the per-message times added together (how long the row took).
- **mean** — the average time for one message.
- **p50** — the middle time: half the messages were faster, half slower (median).
- **p95** — almost-worst: only about 5 of every 100 messages were slower (95th percentile).
- **p99** — worst cases: only about 1 of every 100 messages was slower (99th percentile / the occasional hiccup).
- **min** — the single fastest message.
- **max** — the single slowest message (worst hiccup).
- **index** — how many times faster this row is than the slow `per-connection` row. `1.0x` = the baseline, `2.0x` = twice as fast, `48x` = forty-eight times faster.

## Notes (in plain terms)

- **index baseline** — every row is compared to the slow original way (a brand-new
  connection for every single message). The `logjet file` row uses no network, so
  it isn't a fair race — treat its index as "the fastest anything could ever be,"
  not a real speed-up.
- **batch row** — batching sends many messages in one shipment, so the row shows the
  cost *per message* (one shipment's time split across its messages). The note also
  prints how long one whole shipment actually took.
- **async row** — "async" hands the message off and carries on without waiting for
  the network, so the number is just the tiny hand-off cost. The note also prints
  how long we waited at the end for everything to finish (**flush**), how many
  **failed** to send, and how many were **thrown away** because messages were
  produced faster than the network could send them (a safety valve).
- **reuse first/cold call** — the first `reuse` message opens the connection once
  (so it's slower); every message after that reuses it.

## Async backpressure

`lj_logger_log_async` never blocks the caller. Outstanding sends are bounded by a
backpressure policy set via `lj_logger_set_backpressure(logger, model, capacity)`:

- `LJ_BACKPRESSURE_UNBOUNDED` — spawn every send (risk: memory under load).
- `LJ_BACKPRESSURE_DROP` — bounded to `capacity` in-flight; drop + count when full.
- `LJ_BACKPRESSURE_BLOCK` — bounded; block the caller until a slot frees.

Default is `DROP` with capacity `1024`. Observe behavior with
`lj_logger_async_errors`, `lj_logger_async_dropped`, and `lj_logger_async_inflight`,
and drain in-flight sends with `lj_logger_flush(logger, timeout_ms)` (also done
on `lj_logger_free`).

The driver is Rust but calls the exported C ABI symbols of `liblogjet` — the
same functions a C/C++ caller exercises through the shared library — while also
linking the `logjet` crate directly for the file row.

## Run

From this directory:

```bash
./run-demo.sh                # 1000 records per phase, batch=100 (defaults)
./run-demo.sh 5000           # custom record count
./run-demo.sh 1000 50        # low-mem batch size
./run-demo.sh 1000 1000      # hi-mem batch size
```

Arguments are `./run-demo.sh [count] [batch_size]`. The batch size is the number
of records sent per `lj_logger_log_batch()` call (the caller controls it); change
it to see how the per-record amortized cost in the `batch=N` row moves.

The script builds `ljd`, `liblogjet`, and the driver, starts two file-backed
`ljd` instances (OTLP/gRPC on `127.0.0.1:4317`, OTLP/HTTP on `127.0.0.1:4318`),
runs the benchmark (gRPC table then HTTP table), and stops both.

The driver itself takes `benchmark-clib <grpc_endpoint> <count> <batch_size>
[http_endpoint]`; the HTTP table is printed only when an HTTP endpoint is given.

## Extending

Reuse (Ticket 1), batching (Ticket 2), async (Ticket 3), and HTTP keep-alive
(Ticket 4) rows are in place. When further ABI lands, add a phase that calls the
new symbol and a row to the table; the harness is structured for it.
