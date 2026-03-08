#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$SCRIPT_DIR/../../target/debug"
LOGJETD="$TARGET_DIR/logjetd"
EMITTER="$TARGET_DIR/otlp-bofh-grpc-emitter"
CONFIG="$SCRIPT_DIR/logjetd.conf"

if [ ! -x "$LOGJETD" ]; then
    echo "missing $LOGJETD"
    echo "build it first with: make demo"
    exit 1
fi

if [ ! -x "$EMITTER" ]; then
    echo "missing $EMITTER"
    echo "build it first with: make demo"
    exit 1
fi

cd "$SCRIPT_DIR"

echo "starting logjetd with config $CONFIG"
"$LOGJETD" --config "$CONFIG" &
LOGJETD_PID=$!

cleanup() {
    kill "$LOGJETD_PID" 2>/dev/null || true
}

trap cleanup EXIT INT TERM

sleep 1

echo "starting otlp-bofh-grpc-emitter toward 127.0.0.1:4317"
"$EMITTER" 127.0.0.1:4317
