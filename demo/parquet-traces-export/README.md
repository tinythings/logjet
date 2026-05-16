# Parquet Traces Export Demo

Demo that captures OTLP traces through `ljd`, stores them in a `.logjet` file, and exports that file to Parquet through the external exporter plugin.

It uses:

- `target/debug/ljd`
- `target/debug/traces-emitter`
- `target/debug/ljx`
- `target/debug/libljx_parquet_exporter.so`

## Build

From the project root:

```bash
make demo
```

## Run

From this directory:

```bash
sh ./run-demo.sh
```

The script:

1. starts `ljd` in file mode listening on `127.0.0.1:4318`
2. sends 10 trace batches via `traces-emitter`
3. stops `ljd` after flush
4. exports the resulting `.logjet` file to `./logs/traces.parquet`
5. prints a DuckDB query to inspect the result

## Inspect with DuckDB

```bash
duckdb -c "SELECT signal_type, span_name, span_kind, trace_id, duration_ns FROM read_parquet('./logs/traces.parquet') LIMIT 10;"
```

## Notes

- the demo uses `--force` so reruns overwrite the previous output
- traces are captured as OTLP `ExportTraceServiceRequest` batches and stored raw
- the Parquet exporter flattens each span into one row with trace-specific columns
- the unified schema includes all signal columns; unused columns are null for traces
