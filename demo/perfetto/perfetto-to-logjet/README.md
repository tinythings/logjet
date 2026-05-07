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
2. `tracebox` records 5s of scheduler events (CPU switches, process lifecycle,
   CPU frequency, interrupts) via ftrace.
3. `ljd` loads the perfetto-ingest plugin, which spawns `trace_processor`,
   exports the trace as SQLite, maps every Perfetto table to OTel log records
   (sched slices, thread states, ftrace events, spurious wakeups, instant
   events, counters), and streams them into a `.logjet` spool.
4. `ljx view` opens the spool.

## What You Should See

- Thousands of log lines across multiple types:
  ```
  cpu=7 state=R utid=19  dur=7.2us        ← sched_slice
  state=S dur=12.3us utid=3 cpu=1          ← thread_state
  sched_switch cpu=5                        ← ftrace_event
  spurious_wakeup utid=1                    ← spurious_wakeup
  ```
- Press `Enter` to see full OTel attributes for each record.
- Press `F` for field filter (e.g. filter by `perfetto.sched.cpu` to see only one CPU).
- `/` to search, `q` to quit.

## Troubleshooting

- **0 records**: The trace needs ftrace events — they require root. The script
  uses `sudo tracebox`. If passwordless sudo isn't configured, run `sudo ./run-demo.sh`.
- **Fewer records than expected in ljx view**: Delete stale index cache:
  `rm -rf ~/.cache/ljx && ./run-demo.sh`
