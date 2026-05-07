# linux-data-record

Record a short ftrace-based Perfetto trace on a Linux desktop, then open it in
the trace processor for interactive inspection.

## Prerequisites

Build the Perfetto tools (from workspace root):

```bash
./demo/perfetto/build-perfetto.sh
```

The script finds tools via `PERFETTO_OUT` (default: `perfetto/out/linux_release`).
Set `PERFETTO_TRACE_OUT` to override the output trace path.

## Permissions

ftrace requires root. The script runs `tracebox` with `sudo`. If passwordless
sudo is not configured, run the script with `sudo`:

```bash
sudo ./run-demo.sh
```

Or set up passwordless ftrace: `sudo chown -R $USER /sys/kernel/tracing`.

## Run

```bash
./run-demo.sh
```

## What happens

1. `traced` and `traced_probes` are started in the background.
2. `tracebox` captures 5 seconds of ftrace (scheduler, syscalls) via sudo into a `.pftrace` file.
3. Tools are stopped.
4. `trace_processor_shell` opens the trace in interactive SQL mode.
5. Type `.q` to quit.

## Example queries (in trace processor)

```sql
-- List process names
SELECT DISTINCT name FROM process WHERE name IS NOT NULL;

-- Show thread scheduling slices
SELECT ts, dur, utid, cpu, end_state FROM sched ORDER BY ts LIMIT 20;

-- Count kernel functions seen in ftrace
SELECT name, count(*) FROM ftrace_event GROUP BY name ORDER BY count(*) DESC LIMIT 10;
```
