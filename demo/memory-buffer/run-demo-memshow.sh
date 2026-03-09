#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$SCRIPT_DIR/../../target/debug"
LJD="$TARGET_DIR/ljd"
EMITTER="$TARGET_DIR/otlp-bofh-emitter"
COLLECTOR="$TARGET_DIR/otlp-demo-collector"
FORWARDER="$TARGET_DIR/otlp-wire-forwarder"
CONFIG="$SCRIPT_DIR/ljd-memshow.conf"

for bin in "$LJD" "$EMITTER" "$COLLECTOR" "$FORWARDER"; do
    if [ ! -x "$bin" ]; then
        echo "missing $bin"
        echo "build everything first with: make demo"
        exit 1
    fi
done

cd "$SCRIPT_DIR"

echo "starting ljd with config $CONFIG"
"$LJD" --config "$CONFIG" &
LJD_PID=$!

cleanup() {
    kill "${FORWARDER_PID:-}" 2>/dev/null || true
    kill "${COLLECTOR_PID:-}" 2>/dev/null || true
    kill "${LJD_PID:-}" 2>/dev/null || true
}

trap cleanup EXIT INT TERM

sleep 1

echo "memory layout for this run: 3 kept + 12 rotating tail = 15 total visible messages"
echo "sending 10 startup messages"
for i in 1 2 3 4 5 6 7 8 9 10; do
    "$EMITTER" 127.0.0.1:4318 --once --message "this message #$i must be kept"
done

echo "sending 7 BOFH flood messages"
"$EMITTER" 127.0.0.1:4318 --count 7 --interval-ms 0

echo "starting collector on 127.0.0.1:4320"
"$COLLECTOR" 127.0.0.1:4320 &
COLLECTOR_PID=$!

sleep 1

echo
echo ">>>>>> Connecting wire forwarder to replay listener"
echo
"$FORWARDER" 127.0.0.1:7002 127.0.0.1:4320 15 &
FORWARDER_PID=$!

wait "$FORWARDER_PID"
