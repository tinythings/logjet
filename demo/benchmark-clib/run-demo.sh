#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT_DIR=$(CDPATH= cd -- "$SCRIPT_DIR/../.." && pwd)
TARGET_DIR="$ROOT_DIR/target/debug"
LJD="$TARGET_DIR/ljd"
BENCH="$TARGET_DIR/benchmark-clib"
CONFIG="$SCRIPT_DIR/ljd.conf"
ENDPOINT="127.0.0.1:4317"
COUNT="${1:-1000}"
BATCH_SIZE="${2:-100}"

echo "building ljd, liblogjet, and the benchmark driver"
cargo build -p ljd -p liblogjet -p otlp-demo --bin benchmark-clib

mkdir -p "$SCRIPT_DIR/logs"

echo "starting ljd with file-backed OTLP/gRPC ingest on $ENDPOINT"
(cd "$SCRIPT_DIR" && "$LJD" --config "$CONFIG" serve) &
LJD_PID=$!

cleanup() {
    kill "${LJD_PID:-}" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

sleep 1

echo "running benchmark ($COUNT records per phase, batch=$BATCH_SIZE)"
echo
"$BENCH" "$ENDPOINT" "$COUNT" "$BATCH_SIZE"
