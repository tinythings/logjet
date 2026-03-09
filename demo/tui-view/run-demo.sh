#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$SCRIPT_DIR/../../target/debug"
GENERATOR="$TARGET_DIR/otlp-random-logjet-generator"
LJX="$TARGET_DIR/ljx"
CONFIG="$SCRIPT_DIR/tui-view.conf"

if [ ! -x "$GENERATOR" ] || [ ! -x "$LJX" ]; then
    echo "missing demo binaries"
    echo "build them first with: make demo"
    exit 1
fi

. "$CONFIG"

cd "$SCRIPT_DIR"
mkdir -p logs
rm -f "$OUTPUT_FILE"

echo "generating $COUNT random log entries into $OUTPUT_FILE"
"$GENERATOR" "$OUTPUT_FILE" "$COUNT" "$SEED"

echo
echo "opening ljx view on $OUTPUT_FILE"
exec "$LJX" view "$OUTPUT_FILE"
