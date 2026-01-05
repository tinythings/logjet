# Configuration

`logjetd` reads YAML configuration from:

- `/etc/logjet.conf` by default
- a custom path passed through `-c` or `--config`

## Supported Keys

```yaml
output: buffer          # "buffer" or "file"
buffer.size: 100        # KiB, conflicts with buffer.messages
buffer.messages: 5000   # message count, conflicts with buffer.size
buffer.keep: 1000       # keep first N messages forever
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
- `buffer.messages: unset`
- `buffer.keep: 0`
- `file.path: .`
- `file.size: 100`
- `file.name: bar.logjet`
- `ingest.listen: 127.0.0.1:7001`
- `replay.listen: 0.0.0.0:7002`
- `replay.poll_ms: 250`

## Notes

- sizes are interpreted as KiB
- `buffer.keep` means: keep the first `N` messages in a permanent front jar, then rotate only the later FIFO tail
- set either `buffer.size` or `buffer.messages`, never both
- `buffer.size` limits the rotating in-memory tail by bytes
- `buffer.messages` limits the rotating in-memory tail by message count
- `file.*` settings are ignored unless `output: file`
- `buffer.*` settings are ignored unless `output: buffer`
- `file.path` is treated as a directory, not a full file path
- file mode always keeps everything and only rotates to a new append-only file when `file.size` is exceeded
