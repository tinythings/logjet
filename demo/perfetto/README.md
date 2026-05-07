# Perfetto demos

## Building Perfetto

```bash
# From workspace root — downloads deps and builds all needed tools.
./demo/perfetto/build-perfetto.sh
```

Or pass a custom source path:
```bash
./demo/perfetto/build-perfetto.sh /path/to/perfetto
```

The script installs missing system packages (git, python3, curl, tar), downloads
hermetic GN/Ninja/clang toolchain, and builds `trace_processor_shell`, `traced`,
`traced_probes`, and `tracebox` into `perfetto/out/linux_release/`.

Add to PATH or set `LJD_PERFETTO_TRACE_PROCESSOR` to point at
`trace_processor_shell`.

## Demos

- [linux-data-record](linux-data-record/) — capture and inspect a system trace
- [perfetto-to-logjet](perfetto-to-logjet/) — full end-to-end: record ftrace, import via plugin (SQLite export), view in ljx
- [perfetto-to-logjet-rpc](perfetto-to-logjet-rpc/) — same but using RPC stdio mode (no temp files)
