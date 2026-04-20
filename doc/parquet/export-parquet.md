# `ljx --export parquet`

This document covers the current Parquet export path for `ljx`.

The short version:

```text
ljx --export parquet input.logjet -o output.parquet --force
```

That command works only when the Parquet exporter shared library is discoverable
by `ljx`.

## What this does

- reads one `.logjet` input sequentially
- loads the Parquet exporter plugin through the stable exporter ABI
- streams records into the plugin incrementally
- writes the resulting Parquet file through host-owned output callbacks
- preserves input record order in the logical output row stream

This is an **export** path, not a replay or ingest path.
`logjet` remains the storage/replay format.
Parquet is the query/analytics format.

## Build the pieces

From the project root:

```bash
cargo build -p ljx -p ljx-parquet-exporter
```

That produces roughly:

- `target/debug/ljx`
- `target/debug/libljx_parquet_exporter.so`

Release builds work the same way under `target/release/`.

## Make the plugin discoverable

`ljx` searches exporter plugins in this order:

1. entries from `LJX_EXPORTER_PATH`
2. `./exporters`
3. `<ljx-exe-dir>/exporters`
4. `<ljx-exe-dir>/../lib/logjet/exporters`

### Simplest way

Point `LJX_EXPORTER_PATH` at the built `.so` file directly:

```bash
LJX_EXPORTER_PATH=./target/debug/libljx_parquet_exporter.so \
./target/debug/ljx --export parquet input.logjet -o output.parquet --force
```

### Directory-based install

Or copy/symlink the plugin into a directory searched by `ljx`:

```text
./exporters/libljx_parquet_exporter.so
```

Notes:

- built-in formats beat plugins
- first plugin found for a format wins
- later duplicates are ignored with loader diagnostics
- the host validates exporter ABI major/minor before use

For release packaging, installation layout, security posture, and external
author support expectations, see `doc/parquet/exporter-release.md`.

## Basic usage

Export one file:

```text
ljx --export parquet telemetry.logjet -o telemetry.parquet
```

Overwrite existing output:

```text
ljx --export parquet telemetry.logjet -o telemetry.parquet --force
```

If the output file already exists and `--force` is not set, `ljx` fails clearly.

## Current scope

Current Parquet exporter support is intentionally narrow:

- supported record type: logs only
- supported payload kind: OTLP `ExportLogsServiceRequest` only
- metrics/traces: rejected cleanly as unsupported
- unknown/opaque payload kinds: rejected cleanly as unsupported

That narrow first slice is deliberate. Better one honest path than ten mushy ones.

## Produced Parquet schema

The current exporter writes a stable core-column schema.

| column | type | meaning |
|---|---|---|
| `sequence` | `UINT64` | original logjet sequence number |
| `timestamp_unix_ns` | `UINT64` | top-level logjet timestamp |
| `observed_timestamp_unix_ns` | `UINT64?` | OTLP observed timestamp when present |
| `trace_id` | `UTF8?` | lowercase hex trace id |
| `span_id` | `UTF8?` | lowercase hex span id |
| `trace_flags` | `UINT32?` | OTLP trace flags when present |
| `severity_number` | `INT32?` | OTLP severity number |
| `severity_text` | `UTF8?` | OTLP severity text |
| `body_kind` | `UTF8` | body type discriminator |
| `body_string` | `UTF8?` | body text when OTLP body is a string |
| `body_json` | `UTF8?` | stable JSON text for non-string bodies |
| `service_name` | `UTF8?` | `resource.attributes[service.name]` when present |
| `scope_name` | `UTF8?` | instrumentation scope name |
| `scope_version` | `UTF8?` | instrumentation scope version |
| `resource_attributes_json` | `UTF8?` | resource attributes as stable JSON text |
| `scope_attributes_json` | `UTF8?` | scope attributes as stable JSON text |
| `log_attributes_json` | `UTF8?` | log record attributes as stable JSON text |
| `event_name` | `UTF8?` | OTLP log event name when present |

### Why the schema looks like this

The exporter keeps a stable analytics-friendly core schema while avoiding
unbounded column explosion.

So:

- important correlation fields get dedicated columns
- resource attributes stay separate from log attributes
- instrumentation scope stays separate too
- long-tail attributes are preserved as JSON text instead of becoming endless
  dynamic top-level columns
- body type is preserved with `body_kind`
- string bodies get a fast-path in `body_string`
- non-string bodies are preserved in a loss-minimising `body_json` column

## Compression and buffering

Current defaults:

- compression: `zstd`
- bounded buffering: row-group-scale only
- default row-group target: plugin default currently `8192` rows

Internally the plugin buffers only the current batch/row-group scale state and
flushes Parquet row groups incrementally.
It does **not** buffer the whole dataset.

## Plugin options

The Parquet plugin currently understands these init-option keys:

- `output.row-group-rows`
- `output.compression`

Current `ljx` top-level CLI does **not** yet expose plugin-specific option
forwarding, so these are plugin capabilities rather than stable end-user CLI
flags today.

That means the public user-facing command for now is still just:

```text
ljx --export parquet input.logjet -o output.parquet [--force]
```

## Failure modes

Expect direct errors for:

- plugin not found
- plugin ABI mismatch
- unsupported record type or payload kind
- output creation failure
- protobuf decode failure inside the plugin
- Parquet writer failure
- host callback write/flush failure
- finish/finalisation failure

Important output behaviour:

- overwrite is explicit via `--force`
- partial output on failure is possible
- no atomic replace promise is made

That is intentional. Better explicit than magical lies.

## Compatibility notes

The host/plugin boundary is the stable C ABI documented in:

- `doc/parquet/exporters-abi.md`
- `doc/parquet/exporters-data-model.md`

Compatibility rules:

- ABI major must match
- host accepts plugin minor versions less than or equal to the host-supported minor
- schema is intended to stay stable for this exporter line across runs unless a
  documented breaking change is made
- host and plugin may be built by different stable Rust toolchains because the
  runtime boundary is plain C ABI only

Practical constraints:

- host and plugin still need matching runtime platform expectations
- same OS/architecture is required
- glibc vs musl style mismatches are not promised to work
- plugin must be a `cdylib` exporting `ljx_exporter_descriptor_v1`

The plugin is a separately compiled `.so`, not a Rust trait object or an in-host
module.

For a real separate-toolchain smoke test, use:

```text
bash scripts/test-exporter-abi-matrix.sh
```

If a plugin is incompatible, `ljx` ignores it during discovery and reports the
reason through loader diagnostics; explicitly requesting that format then fails
cleanly with those notes included.

## Quick DuckDB check

If you have DuckDB installed, a quick sanity query looks like:

```sql
SELECT sequence, timestamp_unix_ns, severity_text, service_name, body_string
FROM read_parquet('output.parquet')
ORDER BY sequence
LIMIT 20;
```

And for attribute inspection:

```sql
SELECT service_name, log_attributes_json
FROM read_parquet('output.parquet')
WHERE severity_text IS NOT NULL
LIMIT 10;
```

## Related files

- ABI contract: `doc/parquet/exporters-abi.md`
- Data model contract: `doc/parquet/exporters-data-model.md`
- Release hardening and packaging: `doc/parquet/exporter-release.md`
- Plugin implementation: `plugins/parquet-exporter/src/lib.rs`
- Demo material: `demo/parquet-export/README.md`
