# C-API Benchmark (Evidence)

Decomposes the per-record cost of writing OTEL records through the `liblogjet`
C API (`log_record()` / `lj_logger_log`): the per-connection path is slow, and
this demo shows where the time goes.

Reference: [`demo/cpp-shared-lib`](../cpp-shared-lib)

## What it measures

The driver times each call across five phases and prints a table with real
numbers (mean, p50, p95, p99, min, max) plus an `index` column — the improvement
factor `per-connection mean / row mean` (baseline = per-connection; the
`logjet file` row's index is a no-network reference floor):

1. **logjet file (`LogjetWriter::push`)** — appending one record straight to a
   `.logjet` file. No network. Isolates the storage/format layer.
2. **backend OTLP/gRPC (per-connection)** — one `lj_logger_log()` send to `ljd`.
   This is the slow path: a fresh connection per record.
3. **backend OTLP/gRPC (reuse)** — one `lj_logger_log_reuse()` send, reusing a
   persistent gRPC channel (Ticket 1).
4. **backend OTLP/gRPC (batch=N)** — `lj_logger_log_batch()` sending N records in
   one request over the reused channel (Ticket 2). Reported per-record amortized.
5. **backend OTLP/gRPC (async enqueue)** — `lj_logger_log_async()` hands the send
   to a background runtime and returns immediately (Ticket 3). The row is the
   caller-thread enqueue cost; the demo then calls `lj_logger_flush()` and prints
   the async error/dropped counters.

This makes the finding explicit: the file write is cheap (µs); the
per-connection backend send is the expensive part (~ms); connection reuse helps,
batching brings the per-record cost down toward the file-write cost, and async
removes the network round-trip from the caller thread entirely.

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

The script builds `ljd`, `liblogjet`, and the driver, starts file-backed `ljd`
on `127.0.0.1:4317`, runs the benchmark, prints the table, and stops `ljd`.

## Extending

Reuse (Ticket 1), batching (Ticket 2), and async (Ticket 3) rows are in place.
When further ABI lands, add a phase that calls the new symbol and a row to the
table; the harness is structured for it.
