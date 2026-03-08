#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$SCRIPT_DIR/../../target/debug"
LOGJETD="$TARGET_DIR/logjetd"
COLLECTOR="$TARGET_DIR/otlp-demo-collector"
CONFIG="$SCRIPT_DIR/remote-logjetd.conf"

for bin in "$LOGJETD" "$COLLECTOR"; do
    if [ ! -x "$bin" ]; then
        echo "missing $bin"
        echo "build everything first with: make demo"
        exit 1
    fi
done

cd "$SCRIPT_DIR"

echo "starting colourful collector on 127.0.0.1:4320"
"$COLLECTOR" 127.0.0.1:4320 &
COLLECTOR_PID=$!

cleanup() {
    kill "${BRIDGE_PID:-}" 2>/dev/null || true
    kill "${COLLECTOR_PID:-}" 2>/dev/null || true
}

trap cleanup EXIT INT TERM

sleep 1

run_pass() {
    label="$1"
    echo
    echo "===== $label ====="
    echo "starting bridge in drain mode using $CONFIG"
    "$LOGJETD" --config "$CONFIG" bridge &
    BRIDGE_PID=$!
    sleep 4
    kill "$BRIDGE_PID" 2>/dev/null || true
    wait "$BRIDGE_PID" 2>/dev/null || true
    BRIDGE_PID=
    sleep 2
}

run_pass "FIRST DRAIN PASS"
run_pass "SECOND DRAIN PASS"

echo
echo "done; the second pass should not show BOOT MESSAGE #1, #2, or #3 again"
