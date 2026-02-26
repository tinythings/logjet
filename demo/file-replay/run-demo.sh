#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$SCRIPT_DIR/../../target/debug"
COLLECTOR="$TARGET_DIR/otlp-demo-collector"
LOGJETD="$TARGET_DIR/logjetd"
CONFIG="$SCRIPT_DIR/logjetd.conf"

if [ ! -x "$COLLECTOR" ]; then
    echo "missing $COLLECTOR"
    echo "build it first with: make demo"
    exit 1
fi

if [ ! -x "$LOGJETD" ]; then
    echo "missing $LOGJETD"
    echo "build it first with: make demo"
    exit 1
fi

if [ ! -f "$CONFIG" ]; then
    echo "missing $CONFIG"
    exit 1
fi

if [ ! -d "$SCRIPT_DIR/logs" ]; then
    echo "missing $SCRIPT_DIR/logs"
    echo "copy or move the logs directory from ../logjet-file first"
    exit 1
fi

echo "starting OTLP collector on 127.0.0.1:4318"
"$COLLECTOR" 127.0.0.1:4318 &
COLLECTOR_PID=$!

cleanup() {
    kill "$COLLECTOR_PID" 2>/dev/null || true
}

trap cleanup EXIT INT TERM

sleep 1

echo "replaying bofh.logjet from ./logs using collector.url from $CONFIG"
"$LOGJETD" --config "$CONFIG" replay --path "./logs" --name "bofh.logjet"
