# Perfetto-to-logjet (RPC)

Same as `perfetto-to-logjet/` but uses trace processor's `server stdio` RPC
mode instead of SQLite export. No temp files — queries go directly over stdin/stdout.

## Build First

```bash
make dev
./demo/perfetto/build-perfetto.sh
```

## Run

```bash
cd demo/perfetto/perfetto-to-logjet-rpc
./run-demo.sh
```

## What Happens

1. `tracebox` records 5s of scheduler events (CPU switches, process lifecycle,
   CPU frequency, interrupts) via ftrace.
2. `ljd` loads the perfetto-ingest plugin with `LJD_PERFETTO_ACQUISITION=rpc`
   set via `ingest.plugin-env` config.
3. The plugin spawns a fresh `trace_processor server stdio` for each query,
   sends SQL, receives protobuf responses, maps rows to OTel log records,
   and streams them into a `.logjet` spool — no temp SQLite files.
4. `ljx view` opens the spool.

## What You Should See

Same output as the SQLite demo — thousands of log lines across multiple types
(sched slices, thread states, ftrace events, spurious wakeups).

## SQLite vs RPC

| | SQLite (default) | RPC |
|---|---|---|
| Config | `ingest.plugin-path: ...` | + `ingest.plugin-env: LJD_PERFETTO_ACQUISITION=rpc` |
| Temp files | Yes (SQLite export) | No |
| Speed | Faster (single trace load) | Slower (one load per query type) |
| Maturity | Stable | New |

## Troubleshooting

- **0 records**: Run with `sudo ./run-demo.sh` for ftrace access.
- **ljx view shows fewer records**: Delete stale index cache: `rm -rf ~/.cache/ljx`.
