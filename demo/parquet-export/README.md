# Parquet Export Demo

Small demo for the `ljx --export parquet` flow.

It generates a `.logjet` file with about 5K BOFH log records and then exports
that file to Parquet through the external exporter plugin.

It uses:

- `target/debug/otlp-bofh-logjet-generator`
- `target/debug/ljx`
- `target/debug/libljx_parquet_exporter.so`

## Build

From the project root:

```bash
cargo build -p otlp-demo --bin otlp-bofh-logjet-generator -p ljx -p ljx-parquet-exporter
```

## Run

From this directory:

```bash
sh ./run-demo.sh
```

The script:

1. checks that the BOFH generator, `ljx`, and the Parquet exporter plugin were built
2. generates `./out/bofh-5000.logjet` with about 5K BOFH records by default
3. points `LJX_EXPORTER_PATH` at the plugin `.so`
4. exports that `.logjet` file to `./out/bofh-5000.parquet`
5. prints where the Parquet file landed

Set `COUNT` if you want a different dataset size:

```bash
COUNT=7500 sh ./run-demo.sh
```

## Inspect with DuckDB

Optional:

```bash
duckdb ./out/demo.duckdb
```

Then:

```sql
SELECT sequence, severity_text, service_name, body_string
FROM read_parquet('./out/bofh-5000.parquet')
ORDER BY sequence
LIMIT 20;
```

## Notes

- the demo uses `--force` so reruns overwrite the previous output
- reruns also regenerate the `.logjet` input before exporting
- this is export only; it does not load data back into logjet
- exporter plugins are searched via `LJX_EXPORTER_PATH`, `./exporters`, paths relative to the `ljx` executable, and on Unix in `/usr/lib/logjet/exporters` and `/usr/lib/logjet`
- if plugin discovery breaks, try printing `LJX_EXPORTER_PATH` first
