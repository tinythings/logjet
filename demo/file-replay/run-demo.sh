#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$SCRIPT_DIR/../../target/debug"
COLLECTOR="$TARGET_DIR/otlp-demo-collector"
LJD="$TARGET_DIR/ljd"
GENERATOR="$TARGET_DIR/otlp-bofh-logjet-generator"
CONFIG="$SCRIPT_DIR/logjetd.conf"
LOG_DIR="$SCRIPT_DIR/logs"
LOG_NAME="bofh.logjet"
LOG_PATH="$LOG_DIR/$LOG_NAME"
RECORD_COUNT="${BOFH_RECORD_COUNT:-128}"

if [ ! -x "$COLLECTOR" ]; then
    echo "missing $COLLECTOR"
    echo "build it first with: make demo"
    exit 1
fi

if [ ! -x "$LJD" ]; then
    echo "missing $LJD"
    echo "build it first with: make demo"
    exit 1
fi

if [ ! -x "$GENERATOR" ]; then
    echo "missing $GENERATOR"
    echo "build it first with: make demo"
    exit 1
fi

if [ ! -f "$CONFIG" ]; then
    echo "missing $CONFIG"
    exit 1
fi

mkdir -p "$LOG_DIR"

if [ ! -f "$LOG_PATH" ]; then
    echo "generating $LOG_PATH with $RECORD_COUNT BOFH OTLP log records"
    "$GENERATOR" "$LOG_PATH" "$RECORD_COUNT"
fi

echo "starting OTLP collector on 127.0.0.1:4318"
"$COLLECTOR" 127.0.0.1:4318 &
COLLECTOR_PID=$!

cleanup() {
    kill "$COLLECTOR_PID" 2>/dev/null || true
}

trap cleanup EXIT INT TERM

sleep 1

echo "replaying $LOG_NAME from ./logs using collector.url from $CONFIG"
"$LJD" --config "$CONFIG" replay --path "$LOG_DIR" --name "$LOG_NAME"
