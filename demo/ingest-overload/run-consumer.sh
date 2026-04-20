#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$SCRIPT_DIR/../../target/debug"
COLLECTOR="$TARGET_DIR/otlp-demo-collector"
FORWARDER="$TARGET_DIR/otlp-wire-forwarder"

require_fresh_bin() {
    bin=$1
    shift

    if [ ! -x "$bin" ]; then
        echo "missing $bin"
        echo "build everything first with: make demo"
        exit 1
    fi

    for src in "$@"; do
        if [ "$src" -nt "$bin" ]; then
            echo "stale $bin"
            echo "rebuild it first with: make demo"
            exit 1
        fi
    done
}

require_fresh_bin "$COLLECTOR" \
    "$SCRIPT_DIR/../src/bin/otlp-demo-collector.rs" \
    "$SCRIPT_DIR/../src/lib.rs" \
    "$SCRIPT_DIR/../Cargo.toml"

require_fresh_bin "$FORWARDER" \
    "$SCRIPT_DIR/../src/bin/otlp-wire-forwarder.rs" \
    "$SCRIPT_DIR/../src/lib.rs" \
    "$SCRIPT_DIR/../Cargo.toml"

cd "$SCRIPT_DIR"

echo "starting collector on 127.0.0.1:4320"
"$COLLECTOR" 127.0.0.1:4320 &
COLLECTOR_PID=$!

cleanup() {
    kill "${COLLECTOR_PID:-}" 2>/dev/null || true
}

trap cleanup EXIT INT TERM

sleep 1

echo "draining retained records from 127.0.0.1:7002 into the collector"
"$FORWARDER" 127.0.0.1:7002 127.0.0.1:4320
