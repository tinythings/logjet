#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT_DIR="$SCRIPT_DIR/../../.."
PERFETTO_OUT="${PERFETTO_OUT:-$ROOT_DIR/perfetto/out/linux_release}"
PERFETTO_TRACE_OUT="${PERFETTO_TRACE_OUT:-$SCRIPT_DIR/trace.pftrace}"

TRACED="$PERFETTO_OUT/traced"
TRACED_PROBES="$PERFETTO_OUT/traced_probes"
TRACEBOX="$PERFETTO_OUT/tracebox"
TP="$PERFETTO_OUT/trace_processor_shell"

for bin in "$TRACED" "$TRACED_PROBES" "$TRACEBOX" "$TP"; do
    if [ ! -x "$bin" ]; then
        echo "missing $bin"
        echo "build first from workspace root:"
        echo "  cd perfetto && gn gen out/linux --args='is_debug=false'"
        echo "  ninja -C out/linux trace_processor_shell traced traced_probes tracebox"
        exit 1
    fi
done

echo "Starting traced..."
"$TRACED" &>/dev/null &
TRACED_PID=$!

echo "Starting traced_probes..."
"$TRACED_PROBES" &>/dev/null &
PROBES_PID=$!

cleanup() {
    kill "$TRACED_PID" 2>/dev/null || true
    kill "$PROBES_PID" 2>/dev/null || true
    wait "$TRACED_PID" 2>/dev/null || true
    wait "$PROBES_PID" 2>/dev/null || true
    echo "Services stopped."
}

trap cleanup EXIT INT TERM

sleep 1

echo "Recording 5s of ftrace to $PERFETTO_TRACE_OUT..."
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
            ftrace_events: "power/cpu_frequency"
        }
    }
}
duration_ms: 5000
ENDCONFIG

if [ "$(id -u)" -eq 0 ]; then
    "$TRACEBOX" --txt -c "$CONFIG_FILE" -o "$PERFETTO_TRACE_OUT"
else
    sudo "$TRACEBOX" --txt -c "$CONFIG_FILE" -o "$PERFETTO_TRACE_OUT"
    sudo chown "$(id -u):$(id -g)" "$PERFETTO_TRACE_OUT"
fi

rm -f "$CONFIG_FILE"

cleanup

if [ ! -f "$PERFETTO_TRACE_OUT" ]; then
    echo "Trace file not created."
    exit 1
fi

SIZE=$(du -h "$PERFETTO_TRACE_OUT" | cut -f1)
echo "Trace recorded: $PERFETTO_TRACE_OUT ($SIZE)"
echo ""
echo "Opening in trace processor (type .q to quit)..."
echo ""
"$TP" "$PERFETTO_TRACE_OUT"
