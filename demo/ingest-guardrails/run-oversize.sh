#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$SCRIPT_DIR/../../target/debug"
LJD="$TARGET_DIR/ljd"
EMITTER="$TARGET_DIR/otlp-bofh-emitter"
CONFIG="$SCRIPT_DIR/http-limit.conf"

for bin in "$LJD" "$EMITTER"; do
    if [ ! -x "$bin" ]; then
        echo "missing $bin"
        echo "build everything first with: make demo"
        exit 1
    fi
done

cd "$SCRIPT_DIR"

cleanup() {
    kill "${LJD_PID:-}" 2>/dev/null || true
}

trap cleanup EXIT INT TERM

echo "starting ljd with tiny ingest.max-batch-bytes"
"$LJD" --config "$CONFIG" &
LJD_PID=$!

sleep 1

echo
echo "sending one small OTLP batch that should be accepted"
"$EMITTER" 127.0.0.1:4318 --once --service-name SMALL --message "small batch accepted"

OVERSIZED_PAYLOAD="OVERSIZED $(awk 'BEGIN { for (i = 0; i < 600; i++) printf "X" }')"

echo
echo "sending one oversized OTLP batch that should be rejected"
"$EMITTER" 127.0.0.1:4318 --once --service-name HUGE --message "$OVERSIZED_PAYLOAD" || true

echo
echo "expected result:"
echo "- SMALL is accepted"
echo "- HUGE is rejected with payload too large"
