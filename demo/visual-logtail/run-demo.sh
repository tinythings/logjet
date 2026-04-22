#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$SCRIPT_DIR/../../target/debug"
EMITTER="$TARGET_DIR/visual-logtail-emitter"
LJX="$TARGET_DIR/ljx"
OUTPUT_DIR="$SCRIPT_DIR/logs"
OUTPUT_FILE="$OUTPUT_DIR/visual-logtail.logjet"
EMITTER_LOG="$OUTPUT_DIR/visual-logtail-emitter.log"
SEED=424242

if [ ! -x "$EMITTER" ] || [ ! -x "$LJX" ]; then
    echo "missing demo binaries"
    echo "build them first with: cargo build -p ljx -p otlp-demo --bins"
    exit 1
fi

mkdir -p "$OUTPUT_DIR"
: > "$EMITTER_LOG"

if [ ! -f "$OUTPUT_FILE" ]; then
    : > "$OUTPUT_FILE"
    printf 'created fresh demo file -> %s\n' "$OUTPUT_FILE"
else
    printf 'reusing existing demo file -> %s\n' "$OUTPUT_FILE"
fi

cleanup() {
    if [ -n "${EMITTER_PID:-}" ]; then
        kill "$EMITTER_PID" 2>/dev/null || true
        wait "$EMITTER_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT INT TERM

printf 'starting visual tail emitter -> %s\n' "$OUTPUT_FILE"
printf 'emitter stdout/stderr -> %s\n' "$EMITTER_LOG"
"$EMITTER" "$OUTPUT_FILE" "$SEED" >>"$EMITTER_LOG" 2>&1 &
EMITTER_PID=$!

sleep 1
printf 'opening ljx view --tail on %s\n' "$OUTPUT_FILE"
"$LJX" view --tail "$OUTPUT_FILE"
