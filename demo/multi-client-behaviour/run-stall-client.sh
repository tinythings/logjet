#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$SCRIPT_DIR/../../target/debug"
STALL="$TARGET_DIR/replay-stall-client"

if [ ! -x "$STALL" ]; then
    echo "missing $STALL"
    echo "build everything first with: make demo"
    exit 1
fi

cd "$SCRIPT_DIR"

echo "starting stalled drain-mode replay client"
"$STALL" 127.0.0.1:7002 10000
