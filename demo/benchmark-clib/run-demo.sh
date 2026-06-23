#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT_DIR=$(CDPATH= cd -- "$SCRIPT_DIR/../.." && pwd)
TARGET_DIR="$ROOT_DIR/target/debug"
LJD="$TARGET_DIR/ljd"
BENCH="$TARGET_DIR/benchmark-clib"
CONFIG="$SCRIPT_DIR/ljd.conf"
CONFIG_HTTP="$SCRIPT_DIR/ljd-http.conf"
ENDPOINT="127.0.0.1:4317"
ENDPOINT_HTTP="127.0.0.1:4318"
COUNT="${1:-1000}"
BATCH_SIZE="${2:-100}"

echo "building ljd, liblogjet, and the benchmark driver"
cargo build -p ljd -p liblogjet -p otlp-demo --bin benchmark-clib

mkdir -p "$SCRIPT_DIR/logs"

# Run from the demo dir so ljd resolves the relative file.path (./logs), and start
# ljd directly (no subshell) so the trap can actually kill it.
cd "$SCRIPT_DIR"

echo "starting ljd with file-backed OTLP/gRPC ingest on $ENDPOINT"
"$LJD" --config "$CONFIG" serve &
LJD_PID=$!

echo "starting ljd with file-backed OTLP/HTTP ingest on $ENDPOINT_HTTP"
"$LJD" --config "$CONFIG_HTTP" serve &
LJD_HTTP_PID=$!

cleanup() {
    kill "${LJD_PID:-}" 2>/dev/null || true
    kill "${LJD_HTTP_PID:-}" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

sleep 1

echo "running benchmark ($COUNT records per phase, batch=$BATCH_SIZE)"
echo
"$BENCH" "$ENDPOINT" "$COUNT" "$BATCH_SIZE" "$ENDPOINT_HTTP"
