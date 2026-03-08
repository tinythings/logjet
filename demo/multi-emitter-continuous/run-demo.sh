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
    kill "${ALICE_PID:-}" 2>/dev/null || true
    kill "${BOB_PID:-}" 2>/dev/null || true
    kill "${CAROL_PID:-}" 2>/dev/null || true
    kill "${DAVE_PID:-}" 2>/dev/null || true
    kill "${EVE_PID:-}" 2>/dev/null || true
    kill "${FORWARDER_PID:-}" 2>/dev/null || true
    kill "${COLLECTOR_PID:-}" 2>/dev/null || true
    kill "${LOGJETD_PID:-}" 2>/dev/null || true
}

trap cleanup EXIT INT TERM

sleep 1

echo "starting colourful collector on 127.0.0.1:4320"
"$COLLECTOR" 127.0.0.1:4320 &
COLLECTOR_PID=$!

sleep 1

echo "connecting wire forwarder to replay listener"
"$FORWARDER" 127.0.0.1:7002 127.0.0.1:4320 &
FORWARDER_PID=$!

sleep 1

start_emitter() {
    name="$1"
    interval_ms="$2"
    echo "starting emitter $name"
    "$EMITTER" 127.0.0.1:4318 --service-name "$name" --interval-ms "$interval_ms" &
    eval "${name}_PID=\$!"
}

start_emitter ALICE 700
start_emitter BOB 900
start_emitter CAROL 1100
start_emitter DAVE 1300
start_emitter EVE 1500

echo "five emitters are running; press Ctrl+C to stop"
wait
