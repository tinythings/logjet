# TUI View Demo

This demo generates one `.logjet` file with 1000 randomized OTLP log records
and immediately opens the interactive `ljx view` TUI on that file.

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

1. reads demo settings from `./tui-view.conf`
2. generates 1000 randomized OTLP log records into `./logs/tui-view.logjet`
3. opens `ljx view` on that file

The generated records vary:

- service name
- severity level
- message text
- sequence number

That gives the TUI enough variety to exercise:

- literal-string filtering
- regex filtering
- popup inspection
- navigation over a larger result set

## Files

- `./tui-view.conf`
  - demo settings such as output path, count, and seed
- `./logs/tui-view.logjet`
  - generated input file for `ljx view`
