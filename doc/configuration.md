# Configuration

`logjetd` reads YAML configuration from:

- `/etc/logjet.conf` by default
- a custom path passed through `-c` or `--config`

## Supported Keys

```yaml
output: buffer          # "buffer" or "file"
buffer.size: 100        # KiB
buffer.preserve: 1000   # preserve first N messages
file.path: /foo         # directory, used only when output: file
file.size: 100          # KiB per file segment
file.name: bar.logjet   # base file name
ingest.listen: 127.0.0.1:7001
replay.listen: 0.0.0.0:7002
replay.poll_ms: 250
```

## Defaults

If omitted:

- `output: buffer`
- `buffer.size: 100`
- `buffer.preserve: 0`
- `file.path: .`
- `file.size: 100`
- `file.name: bar.logjet`
- `ingest.listen: 127.0.0.1:7001`
- `replay.listen: 0.0.0.0:7002`
- `replay.poll_ms: 250`

## Notes

- sizes are interpreted as KiB
- `file.*` settings are ignored unless `output: file`
- `buffer.*` settings are ignored unless `output: buffer`
- `file.path` is treated as a directory, not a full file path
