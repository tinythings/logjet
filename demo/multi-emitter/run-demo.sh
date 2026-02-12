#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$SCRIPT_DIR/../../target/debug"
LOGJETD="$TARGET_DIR/logjetd"
EMITTER="$TARGET_DIR/otlp-bofh-emitter"
COLLECTOR="$TARGET_DIR/otlp-demo-collector"
FORWARDER="$TARGET_DIR/otlp-wire-forwarder"
CONFIG="$SCRIPT_DIR/logjetd.conf"

for bin in "$LOGJETD" "$EMITTER" "$COLLECTOR" "$FORWARDER"; do
    if [ ! -x "$bin" ]; then
        echo "missing $bin"
        echo "build everything first with: make demo"
        exit 1
    fi
done

cd "$SCRIPT_DIR"

echo "starting logjetd with config $CONFIG"
"$LOGJETD" --config "$CONFIG" &
LOGJETD_PID=$!

cleanup() {
    kill "${FORWARDER_PID:-}" 2>/dev/null || true
    kill "${COLLECTOR_PID:-}" 2>/dev/null || true
    kill "${LOGJETD_PID:-}" 2>/dev/null || true
}

trap cleanup EXIT INT TERM

sleep 1

emit_once() {
    name="$1"
    "$EMITTER" 127.0.0.1:4318 --once --service-name "$name" --message "I am emitter $name"
}

echo "sending five emitter identity messages"
emit_once Alice
emit_once Bob
emit_once Carol
emit_once Dave
emit_once Eve

echo "starting colourful collector on 127.0.0.1:4320"
"$COLLECTOR" 127.0.0.1:4320 &
COLLECTOR_PID=$!

sleep 1

echo "forwarding retained records from the replay listener"
"$FORWARDER" 127.0.0.1:7002 127.0.0.1:4320 5 &
FORWARDER_PID=$!

wait "$FORWARDER_PID"
