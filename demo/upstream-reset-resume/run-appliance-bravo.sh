#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$SCRIPT_DIR/../../target/debug"
LOGJETD="$TARGET_DIR/logjetd"
EMITTER="$TARGET_DIR/otlp-bofh-emitter"
CONFIG="$SCRIPT_DIR/appliance-logjetd.conf"

for bin in "$LOGJETD" "$EMITTER"; do
    if [ ! -x "$bin" ]; then
        echo "missing $bin"
        echo "build everything first with: make demo"
        exit 1
    fi
done

cd "$SCRIPT_DIR"

echo "starting BRAVO appliance stream with a fresh in-memory upstream"
"$LOGJETD" --config "$CONFIG" &
LOGJETD_PID=$!

cleanup() {
    kill "${LOGJETD_PID:-}" 2>/dev/null || true
}

trap cleanup EXIT INT TERM

sleep 1

seq_no=1
while :; do
    message=$(printf 'BRAVO %03d: this is a fresh upstream stream after reset' "$seq_no")
    "$EMITTER" 127.0.0.1:4318 --once --service-name "bravo-emitter" --message "$message"
    seq_no=$((seq_no + 1))
    sleep 1
done
