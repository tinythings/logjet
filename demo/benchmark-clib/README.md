# C-API Benchmark (Evidence)

Decomposes the per-record cost of writing OTEL records through the `liblogjet`
C API (`log_record()` / `lj_logger_log`): the per-connection path is slow, and
this demo shows where the time goes.

Reference: [`demo/cpp-shared-lib`](../cpp-shared-lib)

## What it measures

The driver times each call across four phases and prints a table with real
numbers (mean, p50, p95, p99, min, max):

1. **logjet file (`LogjetWriter::push`)** — appending one record straight to a
   `.logjet` file. No network. Isolates the storage/format layer.
2. **backend OTLP/gRPC (per-connection)** — one `lj_logger_log()` send to `ljd`.
   This is the slow path: a fresh connection per record.
3. **backend OTLP/gRPC (reuse)** — one `lj_logger_log_reuse()` send, reusing a
   persistent gRPC channel (Ticket 1).
4. **backend OTLP/gRPC (batch=N)** — `lj_logger_log_batch()` sending N records in
   one request over the reused channel (Ticket 2). Reported per-record amortized.

This makes the finding explicit: the file write is cheap (µs); the
per-connection backend send is the expensive part (~ms); connection reuse helps,
and batching brings the per-record cost back down toward the file-write cost.

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

Reuse (Ticket 1) and batching (Ticket 2) rows are in place. When further ABI
lands (e.g. async, Ticket 3), add a phase that calls the new symbol and a row to
the table; the harness is structured for it.
