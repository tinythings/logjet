#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$SCRIPT_DIR/../../target/debug"
LJD="$TARGET_DIR/ljd"
EMITTER="$TARGET_DIR/otlp-bofh-emitter"
CONFIG="$SCRIPT_DIR/appliance-logjetd.conf"

for bin in "$LJD" "$EMITTER"; do
    if [ ! -x "$bin" ]; then
        echo "missing $bin"
        echo "build everything first with: make demo"
        exit 1
    fi
done

cd "$SCRIPT_DIR"

echo "starting appliance-side ljd with config $CONFIG"
"$LJD" --config "$CONFIG" &
LJD_PID=$!

cleanup() {
    kill "${LJD_PID:-}" 2>/dev/null || true
}

trap cleanup EXIT INT TERM

sleep 1

echo "sending preserved startup messages"
for i in 1 2 3; do
    "$EMITTER" 127.0.0.1:4318 --once --message "BOOT MESSAGE #$i"
done

echo "sending extra startup tail messages"
for i in 4 5 6; do
    "$EMITTER" 127.0.0.1:4318 --once --message "BOOT MESSAGE #$i"
done

echo "starting continuous BOFH traffic toward appliance-side ljd"
"$EMITTER" 127.0.0.1:4318 --interval-ms 700
