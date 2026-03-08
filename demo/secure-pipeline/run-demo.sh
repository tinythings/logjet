#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$SCRIPT_DIR/../../target/debug"
LOGJETD="$TARGET_DIR/logjetd"
EMITTER="$TARGET_DIR/otlp-bofh-emitter"
COLLECTOR="$TARGET_DIR/otlp-demo-collector"
CONFIG="$SCRIPT_DIR/logjetd.conf"

for bin in "$LOGJETD" "$EMITTER" "$COLLECTOR"; do
    if [ ! -x "$bin" ]; then
        echo "missing $bin"
        echo "build everything first with: make demo"
        exit 1
    fi
done

for file in \
    "$SCRIPT_DIR/certs/ca.pem" \
    "$SCRIPT_DIR/certs/ingest.pem" \
    "$SCRIPT_DIR/certs/ingest.key" \
    "$SCRIPT_DIR/certs/collector.pem" \
    "$SCRIPT_DIR/certs/collector.key"
do
    if [ ! -f "$file" ]; then
        echo "missing $file"
        exit 1
    fi
done

mkdir -p "$SCRIPT_DIR/logs"
cd "$SCRIPT_DIR"

echo "cleaning previous secure demo log files"
rm -f "$SCRIPT_DIR"/logs/secure.logjet "$SCRIPT_DIR"/logs/secure-*.logjet

echo "starting HTTPS collector on 127.0.0.1:4321"
"$COLLECTOR" 127.0.0.1:4321 --tls \
    --cert-file "$SCRIPT_DIR/certs/collector.pem" \
    --key-file "$SCRIPT_DIR/certs/collector.key" &
COLLECTOR_PID=$!

echo "starting logjetd with HTTPS OTLP ingest and HTTPS collector export"
"$LOGJETD" --config "$CONFIG" &
LOGJETD_PID=$!

cleanup() {
    kill "${LOGJETD_PID:-}" 2>/dev/null || true
    kill "${COLLECTOR_PID:-}" 2>/dev/null || true
}

trap cleanup EXIT INT TERM

sleep 1

echo "sending 5 BOFH batches to https://127.0.0.1:4319/v1/logs"
"$EMITTER" https://127.0.0.1:4319/v1/logs \
    --ca-file "$SCRIPT_DIR/certs/ca.pem" \
    --server-name ingest.demo.logjet \
    --count 5 \
    --interval-ms 0

sleep 1

echo "replaying stored records to HTTPS collector"
"$LOGJETD" --config "$CONFIG" replay --path ./logs --name secure.logjet
