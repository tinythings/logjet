# multi-signal-view

Ingest logs, metrics, and traces (all over OTLP/HTTP) into a single `ljd` instance, then open `ljx view` to verify all three signals decode correctly side-by-side.

## Run

```bash
make demo
cd demo/multi-signal-view
./run-demo.sh
```

The demo:
1. Starts `ljd` with OTLP/HTTP ingest on `127.0.0.1:4318`
2. Emits 6 batches per signal (logs → metrics → traces), interleaved, via `multi-signal-emitter`
3. Stops `ljd` after flush
4. Opens `ljx view` on the resulting `.logjet` file
5. Cleans up after the viewer exits

## What to look for in `ljx view`

- **Logs rows**: show BOFH excuse text (body preview)
- **Metrics rows**: show `cpu.usage=N%` and `requests.total=N` summaries
- **Traces rows**: show `GET /api/items/N?page=M` span names with kind
- All three record types coexist in one file in arrival order
- Press `Enter` on any row → full decoded payload
- Press `i` → info panel with signal-specific metadata
