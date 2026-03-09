#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$SCRIPT_DIR/../../target/debug"
LJD="$TARGET_DIR/ljd"
EMITTER="$TARGET_DIR/otlp-bofh-emitter"
CONFIG="$SCRIPT_DIR/logjetd.conf"

for bin in "$LJD" "$EMITTER"; do
    if [ ! -x "$bin" ]; then
        echo "missing $bin"
        echo "build everything first with: make demo"
        exit 1
    fi
done

cd "$SCRIPT_DIR"

echo "starting appliance-side ljd with replay.client-timeout-ms=3000"
"$LJD" --config "$CONFIG" &
LJD_PID=$!

cleanup() {
    kill "${LJD_PID:-}" 2>/dev/null || true
}

trap cleanup EXIT INT TERM

sleep 1

seq_no=1
while :; do
    message=$(printf 'MULTI %03d: normal clients should continue even if another replay client stalls' "$seq_no")
    "$EMITTER" 127.0.0.1:4318 --once --service-name "multi-client-emitter" --message "$message"
    seq_no=$((seq_no + 1))
    sleep 1
done
