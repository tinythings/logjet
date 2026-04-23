# Logcat Loading Demo

Demonstrates ljd loading an **active-source** ingest plugin.

The logcat plugin reads Android `logcat` output from stdin, parses
`threadtime` and `brief` formats, and delivers parsed records directly
into the spool — no TCP listener, no OTLP.

## Build First

```bash
make demo
```

That gives you:

- `target/debug/ljd`
- `target/debug/liblj_logcat_ingest.so`

## Run

```bash
./run-demo.sh
```

The script:

1. pipes 16 fake logcat lines (Futurama quotes) into ljd's stdin
2. ljd dlopen's the logcat plugin `.so`
3. plugin reads stdin, parses each line, fires callback
4. records are stored in `logs/logcat.logjet`
5. opens `ljx view` on the result

## Real Android Usage

Pipe `adb logcat` into ljd:

```bash
adb logcat | ljd serve -c logcat.conf
```

Or on the Android device itself:

```bash
logcat | ljd serve -c /data/local/logcat.conf
```

## Supported Formats

**threadtime** (default `adb logcat` output):

```
06-11 22:14:15.123  1234  5678 I MyApp   : hello world
```

**brief**:

```
I/MyApp(1234): hello world
```

## Passive vs Active Plugins

- **Passive** (syslog plugin): ljd owns TCP, feeds bytes via `lj_ingest_feed`
- **Active** (logcat plugin): plugin exports `lj_ingest_fetch`, owns I/O, reads from its own source

ljd detects the mode automatically at dlopen time.

`ingest.plugin-path` can point directly at a `.so`, or it can be a bare shared
library filename. Bare filenames are searched in `LJD_INGEST_PLUGIN_PATH`,
`./ingestors`, paths relative to the `ljd` executable, and on Unix in
`/usr/lib/logjet/ingestors` and `/usr/lib/logjet`.

## Notes

- Extracts `logcat.tag`, `logcat.pid`, `logcat.tid` as record attributes
- Maps V/D/I/W/E/F Android levels to OTel severity
- No network involved — stdin straight to `.logjet` file
