#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$SCRIPT_DIR/../../target/debug"
LJD="$TARGET_DIR/ljd"
WIRE_EMITTER="$TARGET_DIR/wire-hold-emitter"
CONFIG="$SCRIPT_DIR/wire-limit.conf"

for bin in "$LJD" "$WIRE_EMITTER"; do
    if [ ! -x "$bin" ]; then
        echo "missing $bin"
        echo "build everything first with: make demo"
        exit 1
    fi
done

cd "$SCRIPT_DIR"

cleanup() {
    kill "${FIRST_PID:-}" 2>/dev/null || true
    kill "${SECOND_PID:-}" 2>/dev/null || true
    kill "${LJD_PID:-}" 2>/dev/null || true
}

trap cleanup EXIT INT TERM

echo "starting ljd with ingest.max-clients: 1"
"$LJD" --config "$CONFIG" &
LJD_PID=$!

sleep 1

echo
echo "starting first wire client; it should keep the only ingest slot"
"$WIRE_EMITTER" 127.0.0.1:7001 --service-name FIRST --hold-ms 4000 &
FIRST_PID=$!

sleep 1

echo
echo "starting second wire client; it should be closed while the first one is still connected"
if "$WIRE_EMITTER" 127.0.0.1:7001 --service-name SECOND --hold-ms 1000; then
    echo "unexpected: SECOND stayed connected long enough to send both records"
else
    echo "expected: SECOND was closed because ingest.max-clients was reached"
fi

wait "$FIRST_PID"

echo
echo "starting third wire client after FIRST is gone; it should be accepted"
"$WIRE_EMITTER" 127.0.0.1:7001 --service-name THIRD --hold-ms 1000

echo
echo "expected result:"
echo "- FIRST stays connected"
echo "- SECOND is closed while FIRST holds the slot"
echo "- THIRD is accepted after FIRST exits"
