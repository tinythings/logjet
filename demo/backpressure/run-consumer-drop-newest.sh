#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$SCRIPT_DIR/../../target/debug"
LOGJETD="$TARGET_DIR/logjetd"
COLLECTOR="$TARGET_DIR/otlp-demo-collector"
CONFIG="$SCRIPT_DIR/consumer-drop-newest.conf"

for bin in "$LOGJETD" "$COLLECTOR"; do
    if [ ! -x "$bin" ]; then
        echo "missing $bin"
        echo "build everything first with: make demo"
        exit 1
    fi
done

cd "$SCRIPT_DIR"

echo "starting slow collector on 127.0.0.1:4320 with 2000 ms delay"
"$COLLECTOR" 127.0.0.1:4320 --delay-ms 2000 &
COLLECTOR_PID=$!

cleanup() {
    kill "${COLLECTOR_PID:-}" 2>/dev/null || true
}

trap cleanup EXIT INT TERM

sleep 1

echo "starting consumer-side bridge in drop-newest mode using $CONFIG"
"$LOGJETD" --config "$CONFIG" bridge
