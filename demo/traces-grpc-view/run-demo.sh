#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$SCRIPT_DIR/../../target/debug"
LJD="$TARGET_DIR/ljd"
EMITTER="$TARGET_DIR/traces-grpc-emitter"
LJX="$TARGET_DIR/ljx"
CONFIG="$SCRIPT_DIR/logjetd.conf"
OUTPUT_DIR="$SCRIPT_DIR/logs"
OUTPUT_FILE="$OUTPUT_DIR/traces.logjet"

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
rm -f "$OUTPUT_FILE" "$OUTPUT_DIR/traces-"*.logjet "$OUTPUT_DIR/traces.stream-id"

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

TRACE_COUNT=12
echo "starting traces-grpc-emitter toward 127.0.0.1:4317 ($TRACE_COUNT batches)"
"$EMITTER" 127.0.0.1:4317 "$TRACE_COUNT"

echo "emitter finished; giving ljd time to flush"
sleep 2

echo "stopping ljd"
kill "$LJD_PID" 2>/dev/null || true
wait "$LJD_PID" 2>/dev/null || true
LJD_PID=""

echo "opening ljx view on $OUTPUT_FILE"
"$LJX" view "$OUTPUT_FILE"

echo "cleaning up demo artefacts"
rm -rf "$OUTPUT_DIR"

echo "done"
