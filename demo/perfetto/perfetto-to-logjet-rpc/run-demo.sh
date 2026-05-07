#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT_DIR="$SCRIPT_DIR/../../.."
BUILD_DIR="$ROOT_DIR/target/debug"
PERFETTO_OUT="${PERFETTO_OUT:-$ROOT_DIR/perfetto/out/linux_release}"
SPOOL_DIR="$SCRIPT_DIR/spool"
TRACE_FILE="$SCRIPT_DIR/trace.pftrace"

LJD="$BUILD_DIR/ljd"
LJX="$BUILD_DIR/ljx"
PLUGIN="$BUILD_DIR/liblj_perfetto_ingest.so"
TRACED="$PERFETTO_OUT/traced"
TRACED_PROBES="$PERFETTO_OUT/traced_probes"
TRACEBOX="$PERFETTO_OUT/tracebox"
TP="$PERFETTO_OUT/trace_processor_shell"

for bin in "$LJD" "$LJX" "$PLUGIN"; do
    if [ ! -e "$bin" ]; then
        echo "missing $bin"
        echo "build first with: make dev"
        exit 1
    fi
done

for bin in "$TRACED" "$TRACED_PROBES" "$TRACEBOX" "$TP"; do
    if [ ! -x "$bin" ]; then
        echo "missing $bin"
        echo "build perfetto first with: ./demo/perfetto/build-perfetto.sh"
        exit 1
    fi
done

# ── Record a trace ────────────────────────────────────────────────────────────

echo "Starting traced..."
"$TRACED" &>/dev/null &
TRACED_PID=$!

echo "Starting traced_probes..."
"$TRACED_PROBES" &>/dev/null &
PROBES_PID=$!

cleanup_trace() {
    kill "$TRACED_PID" 2>/dev/null || true
    kill "$PROBES_PID" 2>/dev/null || true
    wait "$TRACED_PID" 2>/dev/null || true
    wait "$PROBES_PID" 2>/dev/null || true
}

trap cleanup_trace EXIT INT TERM

sleep 1

echo "Recording 5s of ftrace to $TRACE_FILE..."
CONFIG_FILE="$SCRIPT_DIR/trace-config.txt"
cat > "$CONFIG_FILE" <<'ENDCONFIG'
buffers: {
    size_kb: 8192
    fill_policy: RING_BUFFER
}
data_sources: {
    config {
        name: "linux.ftrace"
        ftrace_config {
            ftrace_events: "sched/sched_switch"
            ftrace_events: "sched/sched_waking"
            ftrace_events: "sched/sched_process_exec"
            ftrace_events: "sched/sched_process_fork"
            ftrace_events: "sched/sched_process_exit"
            ftrace_events: "power/cpu_frequency"
            ftrace_events: "power/cpu_idle"
            ftrace_events: "irq/irq_handler_entry"
            ftrace_events: "irq/irq_handler_exit"
        }
    }
}
duration_ms: 5000
ENDCONFIG

if [ "$(id -u)" -eq 0 ]; then
    "$TRACEBOX" --txt -c "$CONFIG_FILE" -o "$TRACE_FILE"
else
    sudo "$TRACEBOX" --txt -c "$CONFIG_FILE" -o "$TRACE_FILE"
    sudo chown "$(id -u):$(id -g)" "$TRACE_FILE"
fi

rm -f "$CONFIG_FILE"

cleanup_trace
trap - EXIT INT TERM

if [ ! -f "$TRACE_FILE" ]; then
    echo "Trace file not created."
    exit 1
fi

SIZE=$(du -h "$TRACE_FILE" | cut -f1)
echo "Trace recorded: $TRACE_FILE ($SIZE)"
echo ""

# ── Import via perfetto-ingest plugin ─────────────────────────────────────────

echo "Importing into .logjet..."
rm -rf "$SPOOL_DIR"
mkdir -p "$SPOOL_DIR"

# Use a temp config so we don't interfere with the user's config.
CONFIG_FILE="$SCRIPT_DIR/ljd-perfetto.conf"
cat > "$CONFIG_FILE" <<EOF
output: file
file.path: "$SPOOL_DIR"
file.size: 10mb
file.name: perfetto.logjet
ingest.protocol: plugin
ingest.plugin-path: "$PLUGIN"
ingest.plugin-env:
  - LJD_PERFETTO_ACQUISITION=rpc
EOF

LJD_PERFETTO_TRACE_FILE="$TRACE_FILE" \
LJD_PERFETTO_TRACE_PROCESSOR="$TP" \
    "$LJD" serve --config "$CONFIG_FILE" &
LJD_PID=$!

cleanup_ljd() {
    kill "$LJD_PID" 2>/dev/null || true
    wait "$LJD_PID" 2>/dev/null || true
    rm -f "$CONFIG_FILE"
}

trap cleanup_ljd EXIT INT TERM

# Poll until records appear (plugin finishes), up to 60s.
echo "Waiting for import..."
elapsed=0
while [ "$elapsed" -lt 60 ]; do
    if [ -f "$SPOOL_DIR/perfetto.logjet" ]; then
        COUNT=$("$LJX" count "$SPOOL_DIR/perfetto.logjet" 2>/dev/null || echo "0")
        if [ "$COUNT" -gt 0 ] 2>/dev/null; then
            echo "Imported $COUNT records into $SPOOL_DIR/perfetto.logjet"
            break
        fi
    fi
    sleep 1
    elapsed=$((elapsed + 1))
done

kill "$LJD_PID" 2>/dev/null || true
wait "$LJD_PID" 2>/dev/null || true
trap - EXIT INT TERM

# ── View the result ───────────────────────────────────────────────────────────

echo "Opening ljx view..."
"$LJX" view "$SPOOL_DIR/perfetto.logjet"
