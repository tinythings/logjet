# Compression Benchmark Demo

Measures block-level compression efficiency with real OTLP payloads.

Runs three ljd instances with different codecs (none, lz4, zstd),
feeds each the same workload from otlp-bofh-emitter, then inspects
the resulting `.logjet` files to compare:

- records per block (batching effectiveness)
- compressed vs uncompressed bytes
- compression ratio
- file size on disk

## Build First

```bash
make demo
```

## Run

```bash
./run-bench.sh
```

The script:

1. starts 3 ljd instances (ports 4320, 4321, 4322) with codec=none, lz4, zstd
2. fires 200 OTLP log records into each
3. waits for flush (200ms flush interval)
4. runs `ljd inspect` on each spool
5. prints a comparison summary

## What to Expect

With 200 records of typical OTLP protobuf (~200–400 bytes each):

- **none:** ratio ≈ 100%, baseline — no compression
- **lz4:** ratio ≈ 40–60%, fast, good for real-time
- **zstd:** ratio ≈ 25–45%, better ratio, more CPU

Records per block should be well above 1 (proves Ticket 2 batching
works). Typical: 50–200 records/block depending on payload size
and 64 KiB block target.

## Notes

- Each ljd instance uses a separate spool directory under `./bench-data/`
- Directories are cleaned before each run
- Segment size is 1 MiB (large enough for 200 records in one segment)
- Block alignment is disabled (0) to see true compressed sizes
