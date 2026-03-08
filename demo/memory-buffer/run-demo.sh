#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$SCRIPT_DIR/../../target/debug"
LOGJETD="$TARGET_DIR/logjetd"
EMITTER="$TARGET_DIR/otlp-bofh-emitter"
COLLECTOR="$TARGET_DIR/otlp-demo-collector"
FORWARDER="$TARGET_DIR/otlp-wire-forwarder"
CONFIG="$SCRIPT_DIR/logjetd.conf"

for bin in "$LOGJETD" "$EMITTER" "$COLLECTOR" "$FORWARDER"; do
    if [ ! -x "$bin" ]; then
        echo "missing $bin"
        echo "build everything first with: make demo"
        exit 1
    fi
done

cd "$SCRIPT_DIR"

echo "starting logjetd with config $CONFIG"
"$LOGJETD" --config "$CONFIG" &
LOGJETD_PID=$!

cleanup() {
    kill "${FORWARDER_PID:-}" 2>/dev/null || true
    kill "${COLLECTOR_PID:-}" 2>/dev/null || true
    kill "${LOGJETD_PID:-}" 2>/dev/null || true
}

trap cleanup EXIT INT TERM

sleep 1

echo "sending 10 startup messages"
for i in 1 2 3 4 5 6 7 8 9 10; do
    "$EMITTER" 127.0.0.1:4318 --once --message "this message #$i must be kept"
done

echo "sending 100 BOFH flood messages"
"$EMITTER" 127.0.0.1:4318 --count 100 --interval-ms 0

echo "starting colorful collector on 127.0.0.1:4320"
"$COLLECTOR" 127.0.0.1:4320 &
COLLECTOR_PID=$!

sleep 1

echo "connecting wire forwarder to replay listener"
"$FORWARDER" 127.0.0.1:7002 127.0.0.1:4320 13 &
FORWARDER_PID=$!

wait "$FORWARDER_PID"
