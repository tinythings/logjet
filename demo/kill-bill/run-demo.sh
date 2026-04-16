#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$SCRIPT_DIR/../../target/debug"
LJD="$TARGET_DIR/ljd"
EMITTER="$TARGET_DIR/otlp-bofh-emitter"
COLLECTOR="$TARGET_DIR/otlp-demo-collector"
CONFIG="$SCRIPT_DIR/logjetd.conf"
LOG_DIR="$SCRIPT_DIR/logs"
DAMAGED_DIR="$SCRIPT_DIR/damaged"
ORIGINAL_FILE="$LOG_DIR/killbill.logjet"
DAMAGED_FILE="$DAMAGED_DIR/killbill.logjet"
EMIT_DELAY_S=0.03

for bin in "$LJD" "$EMITTER" "$COLLECTOR"; do
    if [ ! -x "$bin" ]; then
        echo "missing $bin"
        echo "build everything first with: make demo"
        exit 1
    fi
done

cd "$SCRIPT_DIR"

cleanup() {
    kill "${COLLECTOR_PID:-}" 2>/dev/null || true
    kill "${LJD_PID:-}" 2>/dev/null || true
}

trap cleanup EXIT INT TERM

echo "cleaning previous demo files"
rm -rf "$LOG_DIR" "$DAMAGED_DIR"
mkdir -p "$LOG_DIR" "$DAMAGED_DIR"

echo "starting ljd to write one .logjet file with 100 messages"
echo "pacing writes by ${EMIT_DELAY_S}s so the demo reliably produces multiple recoverable blocks"
"$LJD" --config "$CONFIG" &
LJD_PID=$!

sleep 1

i=1
while [ "$i" -le 100 ]; do
    MESSAGE=$(printf 'KILL BILL %03d: the reader should recover this if its block survives the byte cut' "$i")
    "$EMITTER" 127.0.0.1:4318 --once --service-name KILL-BILL --message "$MESSAGE" >/dev/null
    sleep "$EMIT_DELAY_S"
    i=$((i + 1))
done

sleep 1
kill "$LJD_PID"
wait "$LJD_PID" 2>/dev/null || true
unset LJD_PID

if [ ! -f "$ORIGINAL_FILE" ]; then
    echo "expected $ORIGINAL_FILE to exist"
    exit 1
fi

FILE_SIZE=$(wc -c < "$ORIGINAL_FILE" | tr -d ' ')
CHUNK_SIZE=$((FILE_SIZE / 3))
START_OFFSET=$((FILE_SIZE / 3))

echo
echo "original file size: $FILE_SIZE bytes"
echo "cutting out the middle third: offset=$START_OFFSET size=$CHUNK_SIZE"
if dd --version >/dev/null 2>&1; then
    dd if="$ORIGINAL_FILE" of="$DAMAGED_FILE" bs=1 skip="$START_OFFSET" count="$CHUNK_SIZE" status=none
else
    dd if="$ORIGINAL_FILE" of="$DAMAGED_FILE" bs=1 skip="$START_OFFSET" count="$CHUNK_SIZE" 2>/dev/null
fi

echo
echo "inspecting original file"
"$LJD" inspect "$ORIGINAL_FILE"

echo
echo "inspecting damaged middle-third file"
"$LJD" inspect "$DAMAGED_FILE"

echo
echo "starting collector on 127.0.0.1:4321"
"$COLLECTOR" 127.0.0.1:4321 &
COLLECTOR_PID=$!

sleep 1

echo
echo "replaying only the damaged middle-third file"
"$LJD" --config "$CONFIG" replay --path "$DAMAGED_DIR" --name "killbill.logjet"

echo
echo "point of the demo:"
echo "- the first bytes of the damaged file are not a valid file start"
echo "- ljd scans forward until it finds the next block sync marker"
echo "- records in surviving later blocks are still replayed"
