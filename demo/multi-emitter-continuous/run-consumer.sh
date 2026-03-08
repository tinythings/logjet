#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$SCRIPT_DIR/../../target/debug"
COLLECTOR="$TARGET_DIR/otlp-demo-collector"
FORWARDER="$TARGET_DIR/otlp-wire-forwarder"

for bin in "$COLLECTOR" "$FORWARDER"; do
    if [ ! -x "$bin" ]; then
        echo "missing $bin"
        echo "build everything first with: make demo"
        exit 1
    fi
done

cd "$SCRIPT_DIR"

echo "starting collector on 127.0.0.1:4320"
"$COLLECTOR" 127.0.0.1:4320 &
COLLECTOR_PID=$!

cleanup() {
    kill "${FORWARDER_PID:-}" 2>/dev/null || true
    kill "${COLLECTOR_PID:-}" 2>/dev/null || true
}

trap cleanup EXIT INT TERM

sleep 1

echo "connecting wire forwarder to replay listener"
"$FORWARDER" 127.0.0.1:7002 127.0.0.1:4320 &
FORWARDER_PID=$!

echo "collector side is running; press Ctrl+C here to stop the consumer side"
wait
