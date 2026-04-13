# Plugin Loading Demo

Demonstrates ljd loading an ingest plugin (`.so`) at runtime.

The syslog plugin parses RFC 3164 / RFC 5424 syslog messages from a raw
TCP stream and delivers parsed log records directly into the spool —
no OTLP encoding on the sender side.

## Build First

```bash
make demo
```

That gives you:

- `target/debug/ljd`
- `target/debug/liblj_syslog_ingest.so`

## Run

```bash
./run-demo.sh
```

The script:

1. starts `ljd` with `ingest.protocol: plugin`
2. ljd dlopen's the syslog plugin `.so`
3. listens for raw syslog TCP on `127.0.0.1:5514`
4. sends 16 test syslog messages via `nc`
5. inspects the stored `.logjet` file

## Send Your Own Messages

While the demo is running:

```bash
echo '<13>Oct 11 22:14:15 myhost myapp: hello from syslog' | nc -q0 127.0.0.1 5514
```

## Inspect Output

```bash
../../target/debug/ljd inspect logs/
```

## Config

See `logjetd.conf` — the key fields:

```yaml
ingest.protocol: plugin
ingest.plugin-path: ../../target/debug/liblj_syslog_ingest.so
ingest.listen: 127.0.0.1:5514
```

## Notes

- The plugin receives raw TCP bytes, not OTLP
- Each newline-delimited syslog message becomes one `lj_log_record`
- The plugin extracts severity, facility, hostname, and appname as attributes
- Anyone can write a plugin for any protocol — same C ABI
