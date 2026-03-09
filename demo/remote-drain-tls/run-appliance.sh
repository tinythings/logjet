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

for file in \
    "$SCRIPT_DIR/certs/ca.pem" \
    "$SCRIPT_DIR/certs/appliance.pem" \
    "$SCRIPT_DIR/certs/appliance.key"
do
    if [ ! -f "$file" ]; then
        echo "missing $file"
        exit 1
    fi
done

cd "$SCRIPT_DIR"

echo "starting appliance-side ljd with TLS replay listener and required client certs"
echo "config: $CONFIG"
"$LJD" --config "$CONFIG" &
LJD_PID=$!

cleanup() {
    kill "${LJD_PID:-}" 2>/dev/null || true
}

trap cleanup EXIT INT TERM

sleep 1

echo "sending 8 boot messages"
echo "first 3 are kept forever, next 5 are in the rotating tail"
for i in 1 2 3 4 5 6 7 8; do
    "$EMITTER" 127.0.0.1:4318 --once --message "BOOT MESSAGE #$i"
done

echo "starting continuous BOFH traffic toward appliance-side ljd on 127.0.0.1:4318"
"$EMITTER" 127.0.0.1:4318
