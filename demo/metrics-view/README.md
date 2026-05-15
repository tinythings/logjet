# metrics-view

Generate OTLP metrics, ingest them into `ljd`, and browse the resulting `.logjet` file with `ljx view`.

## What it does

1. Starts `ljd` in file mode listening for OTLP/HTTP on `127.0.0.1:4318`.
2. Runs `metrics-emitter` which POSTs 15 `ExportMetricsServiceRequest` batches to `/v1/metrics`.
   Each batch contains:
   - a Gauge `cpu.usage` with a value that drifts between 10 % and 90 %
   - a cumulative Sum `requests.total` that grows monotonically
3. Stops `ljd` after the emitter finishes.
4. Opens `ljx view` on the stored `metrics.logjet` file.

## Prerequisites

Build the demo binaries:

```bash
make demo
```

## Run

```bash
./run-demo.sh
```

## Keybindings in `ljx view`

- `↑` / `↓` – move through records
- `Enter` – open detail modal for the selected record
- `Esc` – close modal
- `i` – toggle hex/inspection panel
- `q` – quit
