# traces-view

Ingest OTLP/HTTP traces into `ljd`, then open `ljx view` on the result to verify traces decode.

## Run

```bash
make demo
cd demo/traces-view
./run-demo.sh
```

The demo:
1. Starts `ljd` with OTLP/HTTP ingest on `127.0.0.1:4318`
2. Emits 12 trace batches via `traces-emitter` (HTTP `POST /v1/traces`)
3. Stops `ljd` after flush
4. Opens `ljx view` on the resulting `.logjet` file
5. Cleans up after the viewer exits

## What to look for in `ljx view`

- List rows should show span names like `GET /api/items/N?page=M` and span kind
- Press `Enter` to open the modal and see trace IDs, span IDs, parent-child relationships, attributes
- Press `i` to see per-kind and per-status span counts in the info panel
