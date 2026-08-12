# CEL Search CLI Demo

Generates a `.logjet` file with 200 diverse log records and
runs a battery of CEL search queries against them via `ljx`.

## Build First

```bash
make demo
```

## Run

```bash
cd demo/cel-search-cli
./run-demo.sh
```

The script demonstrates CEL searches over:

- severity number / text comparisons
- body substring matching
- service name filtering
- attribute access (`attributes["key"]`)
- resource attribute access (`resource["key"]`)
- event name filtering
- combined AND conditions
- CEL with `--grep` and `--service` flags
- time-range filtering (`--ts-min` / `--ts-max`)
- `ljx count` mode
