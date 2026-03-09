#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$SCRIPT_DIR/../../target/debug"
LJD="$TARGET_DIR/ljd"
EMITTER="$TARGET_DIR/otlp-bofh-grpc-emitter"
CONFIG="$SCRIPT_DIR/logjetd.conf"

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

cd "$SCRIPT_DIR"

echo "starting ljd with config $CONFIG"
"$LJD" --config "$CONFIG" &
LJD_PID=$!

cleanup() {
    kill "$LJD_PID" 2>/dev/null || true
}

trap cleanup EXIT INT TERM

sleep 1

echo "starting otlp-bofh-grpc-emitter toward 127.0.0.1:4317"
"$EMITTER" 127.0.0.1:4317
