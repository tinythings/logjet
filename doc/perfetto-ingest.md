# Perfetto Ingest Plugin (`lj-perfetto-ingest`)

Imports Perfetto trace files (`.pftrace` / `.perfetto-trace`) into the logjet
ecosystem as OTel traces, metrics, logs, and events.

## Architecture

```
 .pftrace ──→ trace_processor (spawned as subprocess)
                 ├── export sqlite  ──→ sqlite_reader ──→ trace_mapper  ──→ OTel spans
                 └── --run-metrics  ──→ metrics_reader ──→ metric_mapper ──→ OTel metrics
                                                              log_mapper   ──→ OTel logs
                                                                                  │
                                                                                  ▼
                                                                           ljd spool (.logjet)
```

The plugin is an **active source** (`mode: 1`). ljd calls `lj_ingest_fetch()` once,
which runs the full pipeline and streams OTel payloads through the generic record
callback.

## Requirements

- Perfetto trace processor binary (`trace_processor` or `trace_processor_shell`).
  Build it from the bundled Perfetto source:
  ```bash
  ./demo/perfetto/build-perfetto.sh
  ```
- A `.pftrace` trace file to import.

## Usage

```bash
# Build the plugin and ljd:
make build

# Run the import:
LJD_PERFETTO_TRACE_FILE=/path/to/trace.pftrace \
LJD_PERFETTO_TRACE_PROCESSOR=/path/to/trace_processor_shell \
  ljd serve \
    --ingest-protocol plugin \
    --ingest-plugin perfetto \
    --storage ./otel-spool
```

## Environment Variables

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `LJD_PERFETTO_TRACE_FILE` | **Yes** | — | Path to the `.pftrace` input file. |
| `LJD_PERFETTO_TRACE_PROCESSOR` | No | PATH search | Path to `trace_processor_shell` binary. |
| `LJD_PERFETTO_TIMESTAMP_POLICY` | No | `best-effort` | `best-effort` or `require-realtime`. |
| `LJD_PERFETTO_METRICS` | No | (none) | Comma-separated metric names to run, e.g. `trace_stats,android_startup`. |

## Output Signals

| Perfetto Source | OTel Signal | Record Type |
|-----------------|-------------|-------------|
| `slice` table | Traces (Spans) | `Traces` |
| Metrics JSON | Metrics (Gauges) | `Metrics` |
| Analysis summary | Logs | `Logs` |
| (reserved) | Events | `Events` |

Spans are batched in groups of 200 per OTLP export request.

## Timestamp Policy

Perfetto timestamps are trace-clock values (typically `CLOCK_MONOTONIC`). The plugin
converts them to Unix epoch nanoseconds using `clock_snapshot` REALTIME entries.

- **best-effort** (default): Spans without realtime data are skipped. Spans before
  the first snapshot are extrapolated backwards.
- **require-realtime**: The pipeline fails if any span cannot be converted.

## Limitations

- **No flow-to-link mapping**: `flow` table entries are read but not yet mapped
  to OTel span links.
- **No args-to-attributes mapping**: Per-slice key-value arguments are read but
  not attached to spans.
- **Thread/process context**: Thread and process names are loaded but not fully
  joined to spans via track relationships.
- **Replay/bridge**: Traces and metrics stored in `.logjet` can be exported via
  `ljx export` but are not yet forwarded by `ljd bridge/replay` (which currently
  only forwards logs).
- **Metrics**: Only scalar metric values are supported (no histograms).
- **Event signal**: The `Events` record type is reserved but not yet generated
  by this plugin.
