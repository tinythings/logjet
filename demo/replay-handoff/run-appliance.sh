#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$SCRIPT_DIR/../../target/debug"
LOGJETD="$TARGET_DIR/logjetd"
EMITTER="$TARGET_DIR/otlp-bofh-emitter"
CONFIG="$SCRIPT_DIR/logjetd.conf"

for bin in "$LOGJETD" "$EMITTER"; do
    if [ ! -x "$bin" ]; then
        echo "missing $bin"
        echo "build everything first with: make demo"
        exit 1
    fi
done

mkdir -p "$SCRIPT_DIR/spool"
cd "$SCRIPT_DIR"

echo "starting logjetd with config $CONFIG"
"$LOGJETD" --config "$CONFIG" serve &
LOGJETD_PID=$!

cleanup() {
    kill "${LIVE_PID:-}" 2>/dev/null || true
    kill "${LOGJETD_PID:-}" 2>/dev/null || true
}

trap cleanup EXIT INT TERM

sleep 1

echo "writing retained backlog before any replay client connects"
"$EMITTER" 127.0.0.1:4318 --once --service-name handoff-demo --message "HANDOFF backlog 001"
"$EMITTER" 127.0.0.1:4318 --once --service-name handoff-demo --message "HANDOFF backlog 002"
"$EMITTER" 127.0.0.1:4318 --once --service-name handoff-demo --message "HANDOFF backlog 003"

echo "starting a live emitter after the backlog is already retained"
"$EMITTER" 127.0.0.1:4318 --service-name handoff-demo --interval-ms 1000 --message "HANDOFF live" &
LIVE_PID=$!

echo "appliance side is running; start ./run-consumer.sh in another terminal"
wait
