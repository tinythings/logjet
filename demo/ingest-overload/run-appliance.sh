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

cd "$SCRIPT_DIR"

echo "starting logjetd with ingest overload policy from $CONFIG"
"$LOGJETD" --config "$CONFIG" &
LOGJETD_PID=$!

cleanup() {
    kill "${LOGJETD_PID:-}" 2>/dev/null || true
}

trap cleanup EXIT INT TERM

sleep 1

echo "sending a fast WARN burst that should mostly be shed"
"$EMITTER" 127.0.0.1:4318 --service-name overload-warn --severity warn --count 8 --interval-ms 0

echo "sending ERROR bursts that must survive overload"
"$EMITTER" 127.0.0.1:4318 --service-name overload-error --severity error --count 3 --interval-ms 0

echo "sending another WARN burst that should still be shed"
"$EMITTER" 127.0.0.1:4318 --service-name overload-warn --severity warn --count 5 --interval-ms 0

echo "appliance side stays up so a consumer can drain retained records"
wait "$LOGJETD_PID"
