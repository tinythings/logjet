# Perfetto Ingest Plugin (`lj-perfetto-ingest`)

Imports Perfetto trace files (`.pftrace` / `.perfetto-trace`) into the logjet
ecosystem as OTel logs, traces, and metrics.

## Architecture

Two acquisition modes — selectable via `ingest.plugin-env` config:

```
 .pftrace ──→ trace_processor (spawned as subprocess)
                ├── SQLite (default): export sqlite → sqlite_reader → mappers
                └── RPC:   server stdio → rpc_reader → mappers
                                                     │
                                                     ▼
                                            buffer & sort by ts
                                                     │
                                                     ▼
                                              ljd spool (.logjet)
```

The plugin is an **active source** (`mode: 1`). ljd calls `lj_ingest_fetch()` once,
which runs the full pipeline. All records from traces, logs, and metrics are
collected, sorted by timestamp, then streamed through the generic record callback
to guarantee monotonic timestamps in the logjet block format.

## Requirements

- Perfetto trace processor binary (`trace_processor` or `trace_processor_shell`).
  Build it from the bundled Perfetto source:
  ```bash
  ./demo/perfetto/build-perfetto.sh
  ```
- A `.pftrace` trace file to import.

## Usage

### SQLite path (default)

```bash
cat > /tmp/perfetto.conf <<EOF
output: file
file.path: ./spool
file.size: 10mb
file.name: perfetto.logjet
ingest.protocol: plugin
ingest.plugin-path: ./target/debug/liblj_perfetto_ingest.so
EOF

LJD_PERFETTO_TRACE_FILE=/path/to/trace.pftrace \
LJD_PERFETTO_TRACE_PROCESSOR=/path/to/trace_processor_shell \
  ljd serve --config /tmp/perfetto.conf
```

### RPC path (no temp SQLite files)

```bash
cat > /tmp/perfetto-rpc.conf <<EOF
output: file
file.path: ./spool
file.size: 10mb
file.name: perfetto.logjet
ingest.protocol: plugin
ingest.plugin-path: ./target/debug/liblj_perfetto_ingest.so
ingest.plugin-env:
  - LJD_PERFETTO_ACQUISITION=rpc
EOF

LJD_PERFETTO_TRACE_FILE=/path/to/trace.pftrace \
LJD_PERFETTO_TRACE_PROCESSOR=/path/to/trace_processor_shell \
  ljd serve --config /tmp/perfetto-rpc.conf
```

`ingest.plugin-env` is a generic ljd config key that passes `KEY=VALUE` pairs
to plugins as environment variables before loading. This avoids plugin-specific
config keys.

See `demo/perfetto/perfetto-to-logjet/run-demo.sh` for a complete end-to-end
example that records, imports, and opens the result in `ljx view`.

## Environment Variables

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `LJD_PERFETTO_TRACE_FILE` | **Yes** | — | Path to the `.pftrace` input file. |
| `LJD_PERFETTO_TRACE_PROCESSOR` | No | PATH search | Path to `trace_processor_shell` binary. |
| `LJD_PERFETTO_TIMESTAMP_POLICY` | No | `best-effort` | `best-effort` or `require-realtime`. |
| `LJD_PERFETTO_ACQUISITION` | No | `sqlite` | `rpc` for stdio RPC mode (no temp SQLite files). |
| `LJD_PERFETTO_METRICS` | No | (none) | Comma-separated metric names to run, e.g. `trace_stats`. |

## Covered Perfetto Types

Every table in the exported SQLite DB has a typed model and DB reader. Types
with data are mapped to OTel log records with structured attributes:

| Perfetto Table | OTel Signal | Attributes |
|---------------|-------------|------------|
| `sched_slice` | Logs | cpu, end_state, dur_ns |
| `thread_state` | Logs | state, dur_ns, cpu, io_wait, blocked_function |
| `ftrace_event` | Logs | name, cpu, utid |
| `spurious_sched_wakeup` | Logs | utid, waker_utid |
| `instant` | Logs | name, track_id |
| `slice` | Traces + Logs | name, dur_ns, depth, parent_id |
| `counter` | Metrics (planned) | value |
| `process`, `thread`, `cpu`, `machine`, `metadata`, `args`, `clock_snapshot` | Metadata | used internally |
| `flow`, `heap_*`, `stack_*`, `memory_*`, `protolog`, `android_logs`, `filedescriptor` | Models ready | not yet mapped |

Each log record carries integer attributes (`perfetto.sched.dur_ns`, etc.) for
structured downstream consumption alongside a human-readable body.

## Timestamp Policy

Perfetto timestamps are trace-clock values (typically `CLOCK_MONOTONIC`). The plugin
converts them to Unix epoch nanoseconds using `clock_snapshot` REALTIME entries.

- **best-effort** (default): Spans without realtime data use extrapolation.
  Spans before the first snapshot are extrapolated backwards.
- **require-realtime**: The pipeline fails if any span cannot be converted.

Records from all mappers are collected into a buffer, sorted by timestamp, then
emitted sequentially. This guarantees monotonicity within logjet blocks even when
different mapper types produce interleaved time ranges.

## Limitations

- **Replay/bridge**: Only logs are forwarded by `ljd bridge/replay`. Traces and
  metrics stored in `.logjet` can be exported via `ljx export` (parquet).
- **ljx view**: Only decodes `ExportLogsServiceRequest` — trace/metric records
  appear as binary data in the detail pane. Trace mapping is disabled by default.
- **Trace span emission**: Currently disabled (ljx can't render it). Enable by
  uncommenting `trace_mapper::map_traces` in `lib.rs`.
- **Histograms**: Metrics only support scalar gauge values.
- **Event signal**: The `Events` record type is reserved but not yet generated.
