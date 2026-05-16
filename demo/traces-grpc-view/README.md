# traces-grpc-view

Ingest OTLP/gRPC traces into `ljd`, then open `ljx view` on the result.

## Run

```bash
make demo
cd demo/traces-grpc-view
./run-demo.sh
```

The demo:
1. Starts `ljd` with OTLP/gRPC ingest on `127.0.0.1:4317`
2. Emits 12 trace batches via `traces-grpc-emitter` (`TraceService/Export`)
3. Stops `ljd` after flush
4. Opens `ljx view` on the resulting `.logjet` file
5. Cleans up after the viewer exits
