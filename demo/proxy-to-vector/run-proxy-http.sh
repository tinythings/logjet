#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT_DIR=$(CDPATH= cd -- "$SCRIPT_DIR/../.." && pwd)
TARGET_DIR="$ROOT_DIR/target/debug"
LJD="$TARGET_DIR/ljd"
EMITTER="$TARGET_DIR/otlp-bofh-emitter"
APPLIANCE_CONFIG="$SCRIPT_DIR/appliance-http-logjetd.conf"
BRIDGE_CONFIG="$SCRIPT_DIR/bridge-http-logjetd.conf"
APPLIANCE_LOG="$SCRIPT_DIR/appliance-http.log"
BRIDGE_LOG="$SCRIPT_DIR/bridge-http.log"

for bin in "$LJD" "$EMITTER"; do
    if [ ! -x "$bin" ]; then
        echo "missing $bin"
        echo "build demo bits first with: make demo"
        exit 1
    fi
done

cd "$SCRIPT_DIR"
rm -f bridge-http.state "$APPLIANCE_LOG" "$BRIDGE_LOG"

cleanup() {
    kill "${EMITTER_PID:-}" 2>/dev/null || true
    kill "${BRIDGE_PID:-}" 2>/dev/null || true
    kill "${APPLIANCE_PID:-}" 2>/dev/null || true
}

trap cleanup EXIT INT TERM

echo "starting HTTP appliance ljd on 127.0.0.1:4319"
"$LJD" --config "$APPLIANCE_CONFIG" >"$APPLIANCE_LOG" 2>&1 &
APPLIANCE_PID=$!

sleep 1

echo "starting HTTP bridge ljd toward Vector on 127.0.0.1:4318"
"$LJD" --config "$BRIDGE_CONFIG" bridge >"$BRIDGE_LOG" 2>&1 &
BRIDGE_PID=$!

sleep 1

echo "HTTP proxy logs:"
echo "  appliance: $APPLIANCE_LOG"
echo "  bridge:    $BRIDGE_LOG"
echo "sending OTLP HTTP logs into appliance ljd on 127.0.0.1:4319"
"$EMITTER" 127.0.0.1:4319 &
EMITTER_PID=$!

wait "$EMITTER_PID"
