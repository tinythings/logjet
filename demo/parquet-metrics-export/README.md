# Parquet Metrics Export Demo

Demo that captures OTLP metrics through `ljd`, stores them in a `.logjet` file, and exports that file to Parquet through the external exporter plugin.

It uses:

- `target/debug/ljd`
- `target/debug/metrics-emitter`
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
2. sends 15 metric batches via `metrics-emitter`
3. stops `ljd` after flush
4. exports the resulting `.logjet` file to `./logs/metrics.parquet`
5. prints a DuckDB query to inspect the result

## Inspect with DuckDB

```bash
duckdb -c "SELECT signal_type, metric_name, metric_type, metric_value_number FROM read_parquet('./logs/metrics.parquet') LIMIT 10;"
```

## Notes

- the demo uses `--force` so reruns overwrite the previous output
- metrics are captured as OTLP `ExportMetricsServiceRequest` batches and stored raw
- the Parquet exporter flattens each datapoint into one row with metric-specific columns
- the unified schema includes all signal columns; unused columns are null for metrics
