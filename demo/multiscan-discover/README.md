# Multiscan Discover Demo

This demo prepares a small tree of `.logjet` files and runs `ljx discover`
against the whole dataset.

It is meant to show:

- JSON stdout for a multi-file `.logjet` dataset
- deterministic directory input and pagination with `--offset` and `--limit`
- summary fields such as matched record count, log event count, time span,
  top services, and severity breakdown
- NDJSON progress output for external tools that want incremental rows
- machine-friendly filters such as `--service`, `--severity`, and `--type`

## Build First

From the project root:

```bash
make demo
```

## Run

From this directory:

```bash
./run-demo.sh
```

## What It Does

The script does this:

1. prints `Preparing the demo data` in bright yellow
2. recreates `./logs`
3. creates three dataset subdirectories
4. writes several `.logjet` files into each subdirectory
5. runs a compact JSON discovery summary over the whole dataset
6. runs a paged JSON discovery summary
7. runs an NDJSON discovery stream filtered to error logs
8. runs a service-filtered summary for one known service

## Commands Shown

Full dataset summary:

```bash
../../target/debug/ljx discover ./logs --type logs --top-services 5
```

Paged summary:

```bash
../../target/debug/ljx discover ./logs --type logs --offset 2 --limit 4
```

Incremental NDJSON rows:

```bash
../../target/debug/ljx discover ./logs --type logs --severity ERROR --limit 5 --ndjson
```

Service-filtered summary:

```bash
../../target/debug/ljx discover ./logs --type logs --service kill-bill --top-services 3
```

## Files

- `./run-demo.sh`
  - prepares the dataset and runs discovery commands
- `./logs/<group>/*.logjet`
  - demo inputs for JSON discovery
