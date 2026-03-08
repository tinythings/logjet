#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$SCRIPT_DIR/../../target/debug"
LOGJETD="$TARGET_DIR/logjetd"
EMITTER="$TARGET_DIR/otlp-bofh-emitter"
COLLECTOR="$TARGET_DIR/otlp-demo-collector"
CONFIG="$SCRIPT_DIR/logjetd.conf"
LOG_DIR="$SCRIPT_DIR/logs"
DAMAGED_DIR="$SCRIPT_DIR/damaged"
ORIGINAL_FILE="$LOG_DIR/killbill.logjet"
DAMAGED_FILE="$DAMAGED_DIR/killbill.logjet"

for bin in "$LOGJETD" "$EMITTER" "$COLLECTOR"; do
    if [ ! -x "$bin" ]; then
        echo "missing $bin"
        echo "build everything first with: make demo"
        exit 1
    fi
done

cd "$SCRIPT_DIR"

cleanup() {
    kill "${COLLECTOR_PID:-}" 2>/dev/null || true
    kill "${LOGJETD_PID:-}" 2>/dev/null || true
}

trap cleanup EXIT INT TERM

echo "cleaning previous demo files"
rm -rf "$LOG_DIR" "$DAMAGED_DIR"
mkdir -p "$LOG_DIR" "$DAMAGED_DIR"

echo "starting logjetd to write one .logjet file with 100 messages"
"$LOGJETD" --config "$CONFIG" &
LOGJETD_PID=$!

sleep 1

i=1
while [ "$i" -le 100 ]; do
    MESSAGE=$(printf 'KILL BILL %03d: the reader should recover this if its block survives the byte cut' "$i")
    "$EMITTER" 127.0.0.1:4318 --once --service-name KILL-BILL --message "$MESSAGE" >/dev/null
    i=$((i + 1))
done

sleep 1
kill "$LOGJETD_PID"
wait "$LOGJETD_PID" 2>/dev/null || true
unset LOGJETD_PID

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
"$LOGJETD" inspect "$ORIGINAL_FILE"

echo
echo "inspecting damaged middle-third file"
"$LOGJETD" inspect "$DAMAGED_FILE"

echo
echo "starting collector on 127.0.0.1:4321"
"$COLLECTOR" 127.0.0.1:4321 &
COLLECTOR_PID=$!

sleep 1

echo
echo "replaying only the damaged middle-third file"
"$LOGJETD" --config "$CONFIG" replay --path "$DAMAGED_DIR" --name "killbill.logjet"

echo
echo "point of the demo:"
echo "- the first bytes of the damaged file are not a valid file start"
echo "- logjetd scans forward until it finds the next block sync marker"
echo "- records in surviving later blocks are still replayed"
