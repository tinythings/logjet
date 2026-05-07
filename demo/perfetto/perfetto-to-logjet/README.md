# Perfetto-to-logjet Demo

End-to-end pipeline: record a Linux ftrace, import it via the perfetto-ingest
plugin into a `.logjet` spool, and view the result in `ljx view`.

## Build First

```bash
# From workspace root
make dev
./scripts/build-perfetto.sh
```

## Run

```bash
cd demo/perfetto/perfetto-to-logjet
./run-demo.sh
```

Requires sudo for ftrace access.

## What Happens

1. `traced` + `traced_probes` start in the background.
2. `tracebox` records 5s of scheduler events (CPU switches) via ftrace.
3. `ljd` loads the perfetto-ingest plugin, which spawns `trace_processor`,
   exports the trace as SQLite, maps `sched_slice` rows to OTel log records
   with CPU/state/duration, and streams them into a `.logjet` spool.
4. `ljx view` opens the spool — each CPU scheduling event appears as one line.

## What You Should See

- Thousands of log lines, each showing a CPU scheduling event:
  ```
  May  7 10:43:15 I  cpu=7 dur=7.2us state=R utid=19 ts=...
  May  7 10:43:15 I  cpu=7 dur=2.0us state=R utid=21 ts=...
  ```
- Press `Enter` to see full OTel attributes (perfetto.sched.id, cpu, end_state).
- Press `F` for field filter, `/` to search, `q` to quit.

## Troubleshooting

- **0 records**: The trace needs ftrace events — they require root. The script
  uses `sudo tracebox`. If passwordless sudo isn't configured, run `sudo ./run-demo.sh`.
- **Fewer records than expected in ljx view**: Delete stale index cache:
  `rm -rf ~/.cache/ljx && ./run-demo.sh`
