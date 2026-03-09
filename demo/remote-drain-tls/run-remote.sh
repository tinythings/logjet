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

for file in \
    "$SCRIPT_DIR/certs/ca.pem" \
    "$SCRIPT_DIR/certs/remote.pem" \
    "$SCRIPT_DIR/certs/remote.key"
do
    if [ ! -f "$file" ]; then
        echo "missing $file"
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

echo "starting remote-side bridge with TLS and client certificate"
echo "it connects to 127.0.0.1:7002 but validates server name appliance.demo.logjet"
echo "config: $CONFIG"
"$LJD" --config "$CONFIG" bridge
