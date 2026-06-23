# C-API Benchmark (Evidence)

Decomposes the per-record cost of writing OTEL records through the `liblogjet`
C API (`log_record()` / `lj_logger_log`): the per-connection path is slow, and
this demo shows where the time goes.

Reference: [`demo/cpp-shared-lib`](../cpp-shared-lib)

## What it measures

The driver times a single call in each of three phases and prints a table with
real numbers (mean, p50, p95, p99, min, max):

1. **logjet file (`LogjetWriter::push`)** — appending one record straight to a
   `.logjet` file. No network. Isolates the storage/format layer.
2. **backend OTLP/gRPC (per-connection)** — one `lj_logger_log()` send to `ljd`.
   This is the slow path: a fresh connection per record.
3. **backend OTLP/gRPC (reuse)** — one `lj_logger_log_reuse()` send, reusing a
   persistent gRPC channel (Ticket 1).

This makes the finding explicit: the file write is cheap (µs); the
per-connection backend send is the expensive part (~ms), and connection reuse
brings it back down toward the file-write cost.

The driver is Rust but calls the exported C ABI symbols of `liblogjet` — the
same functions a C/C++ caller exercises through the shared library — while also
linking the `logjet` crate directly for the file row.

## Run

From this directory:

```bash
./run-demo.sh           # 1000 records per phase (default)
./run-demo.sh 5000      # custom record count
```

The script builds `ljd`, `liblogjet`, and the driver, starts file-backed `ljd`
on `127.0.0.1:4317`, runs the benchmark, prints the table, and stops `ljd`.

## Extending

When new ABI lands (batching, async), add a phase that calls the new symbol and
a row to the table; the harness is structured for it.
