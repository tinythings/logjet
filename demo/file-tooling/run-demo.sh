#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
LOGJETD="$ROOT_DIR/target/debug/logjetd"
EMITTER="$ROOT_DIR/target/debug/otlp-bofh-emitter"
CONFIG="$(cd "$(dirname "$0")" && pwd)/logjetd.conf"
LOG_DIR="$(cd "$(dirname "$0")" && pwd)/logs"

cleanup() {
    if [[ -n "${LOGJETD_PID:-}" ]]; then
        kill "$LOGJETD_PID" >/dev/null 2>&1 || true
        wait "$LOGJETD_PID" 2>/dev/null || true
    fi
}

trap cleanup EXIT

mkdir -p "$LOG_DIR"
rm -f "$LOG_DIR"/ops*.logjet "$LOG_DIR"/ops*.state "$LOG_DIR"/ops*.stream-id

echo "starting file-backed logjetd"
"$LOGJETD" --config "$CONFIG" serve &
LOGJETD_PID=$!
sleep 1

echo "sending enough messages to force file rotation"
"$EMITTER" 127.0.0.1:4318 --service-name file-tooling --count 20 --interval-ms 0

sleep 1

echo
echo "current segment layout"
"$LOGJETD" segments --path "$LOG_DIR" --name ops.logjet

echo
echo "dry-run prune by file count"
"$LOGJETD" prune --path "$LOG_DIR" --name ops.logjet --keep-files 2 --dry-run

echo
echo "dry-run prune by byte budget"
"$LOGJETD" prune --path "$LOG_DIR" --name ops.logjet --keep-bytes 2048 --dry-run

echo
echo "pruning oldest segments and keeping only the newest two files"
"$LOGJETD" prune --path "$LOG_DIR" --name ops.logjet --keep-files 2

echo
echo "segment layout after prune"
"$LOGJETD" segments --path "$LOG_DIR" --name ops.logjet
