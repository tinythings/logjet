# Perfetto demos

These demos assume trace_processor and tracing tools are installed. Build them from the bundled Perfetto source:

```bash
cd perfetto
gn gen out/linux --args='is_debug=false'
ninja -C out/linux trace_processor_shell traced traced_probes tracebox
```

Then add `$PWD/out/linux` to PATH or set env vars (see each demo).
