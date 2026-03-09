#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$SCRIPT_DIR/../../target/debug"
LJD="$TARGET_DIR/ljd"
COLLECTOR="$TARGET_DIR/otlp-demo-collector"
CONFIG="$SCRIPT_DIR/remote-logjetd.conf"

for bin in "$LJD" "$COLLECTOR"; do
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
    kill "${COLLECTOR_PID:-}" 2>/dev/null || true
}

trap cleanup EXIT INT TERM

sleep 1

echo "starting remote-side bridge using $CONFIG"
"$LJD" --config "$CONFIG" bridge
