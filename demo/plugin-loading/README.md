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
5. stops ljd and opens `ljx view` on the stored `.logjet` file

## Send Your Own Messages

While the demo is running:

```bash
echo '<13>Oct 11 22:14:15 myhost myapp: hello from syslog' | nc -q0 127.0.0.1 5514
```

## Capture Real Syslog

Point your system syslog daemon at ljd to capture real logs.

**rsyslog** — add to `/etc/rsyslog.d/logjet.conf`:

```
*.* @@127.0.0.1:5514
```

Then `sudo systemctl restart rsyslog`.

`@@` = TCP (required). `@` = UDP (not supported).

**syslog-ng** — add a destination:

```
destination d_ljd { tcp("127.0.0.1" port(5514)); };
log { source(s_sys); destination(d_ljd); };
```

**journald** (quick and dirty):

```bash
journalctl -f | nc 127.0.0.1 5514
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

`ingest.plugin-path` may be an explicit path, as shown above, or a bare shared
library filename. Bare filenames are searched in `LJD_INGEST_PLUGIN_PATH`,
`./ingestors`, paths relative to the `ljd` executable, and on Unix in
`/usr/lib/logjet/ingestors` and `/usr/lib/logjet`.

## Notes

- The plugin receives raw TCP bytes, not OTLP
- Each newline-delimited syslog message becomes one `lj_log_record`
- The plugin extracts severity, facility, hostname, and appname as attributes
- Anyone can write a plugin for any protocol — same C ABI
