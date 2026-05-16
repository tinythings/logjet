#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$SCRIPT_DIR/../../target/debug"
LJD="$TARGET_DIR/ljd"
EMITTER="$TARGET_DIR/multi-signal-emitter"
LJX="$TARGET_DIR/ljx"
CONFIG="$SCRIPT_DIR/logjetd.conf"
OUTPUT_DIR="$SCRIPT_DIR/logs"
OUTPUT_FILE="$OUTPUT_DIR/mixed.logjet"

if [ ! -x "$LJD" ]; then
    echo "missing $LJD"
    echo "build it first with: make demo"
    exit 1
fi

if [ ! -x "$EMITTER" ]; then
    echo "missing $EMITTER"
    echo "build it first with: make demo"
    exit 1
fi

if [ ! -x "$LJX" ]; then
    echo "missing $LJX"
    echo "build it first with: make demo"
    exit 1
fi

cd "$SCRIPT_DIR"

mkdir -p "$OUTPUT_DIR"
rm -f "$OUTPUT_FILE" "$OUTPUT_DIR/mixed-"*.logjet "$OUTPUT_DIR/mixed.stream-id"

echo "starting ljd with config $CONFIG"
"$LJD" --config "$CONFIG" &
LJD_PID=$!

cleanup() {
    if [ -n "${EMITTER_PID:-}" ]; then
        kill "$EMITTER_PID" 2>/dev/null || true
        wait "$EMITTER_PID" 2>/dev/null || true
    fi
    if [ -n "${LJD_PID:-}" ]; then
        kill "$LJD_PID" 2>/dev/null || true
        wait "$LJD_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT INT TERM

sleep 1

BATCH_COUNT=6
echo "starting multi-signal-emitter toward 127.0.0.1:4318 ($BATCH_COUNT batches per signal)"
"$EMITTER" 127.0.0.1:4318 "$BATCH_COUNT"

echo "emitter finished; giving ljd time to flush"
sleep 2

echo "stopping ljd"
kill "$LJD_PID" 2>/dev/null || true
wait "$LJD_PID" 2>/dev/null || true
LJD_PID=""

echo "opening ljx view on $OUTPUT_FILE"
echo ""
echo "TIP: Navigate through the list. You will see:"
echo "  - BOFH log entries (body text preview)"
echo "  - Metrics entries (cpu.usage=N%, requests.total=N)"
echo "  - Traces entries (GET /api/items/N?page=M)"
echo ""
echo "Press Enter on any row to see the full decoded payload."
echo "Press 'i' for the info panel with signal-specific metadata."
echo ""
"$LJX" view "$OUTPUT_FILE"

echo "cleaning up demo artefacts"
rm -rf "$OUTPUT_DIR"

echo "done"
