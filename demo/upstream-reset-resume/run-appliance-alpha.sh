#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$SCRIPT_DIR/../../target/debug"
LOGJETD="$TARGET_DIR/logjetd"
EMITTER="$TARGET_DIR/otlp-bofh-emitter"
CONFIG="$SCRIPT_DIR/appliance-logjetd.conf"
STATE_FILE="$SCRIPT_DIR/bridge.state"

for bin in "$LOGJETD" "$EMITTER"; do
    if [ ! -x "$bin" ]; then
        echo "missing $bin"
        echo "build everything first with: make demo"
        exit 1
    fi
done

cd "$SCRIPT_DIR"

rm -f "$STATE_FILE"

echo "starting ALPHA appliance stream and clearing old bridge.state"
"$LOGJETD" --config "$CONFIG" &
LOGJETD_PID=$!

cleanup() {
    kill "${LOGJETD_PID:-}" 2>/dev/null || true
}

trap cleanup EXIT INT TERM

sleep 1

seq_no=1
while :; do
    message=$(printf 'ALPHA %03d: this is the first upstream stream' "$seq_no")
    "$EMITTER" 127.0.0.1:4318 --once --service-name "alpha-emitter" --message "$message"
    seq_no=$((seq_no + 1))
    sleep 1
done
