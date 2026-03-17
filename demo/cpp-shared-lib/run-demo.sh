#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$SCRIPT_DIR/../../target/debug"
LJD="$TARGET_DIR/ljd"
LJX="$TARGET_DIR/ljx"
LIB_SRC="$TARGET_DIR/libliblogjet.so"
LIB_DST="$SCRIPT_DIR/liblogjet.so"
CPP_SRC="$SCRIPT_DIR/cpp-logger.cpp"
CPP_BIN="$SCRIPT_DIR/cpp-logger"
CONFIG="$SCRIPT_DIR/ljd.conf"

for bin in "$LJD" "$LJX" "$LIB_SRC"; do
    if [ ! -e "$bin" ]; then
        echo "missing $bin"
        echo "build everything first with: cargo build -p ljd -p ljx -p liblogjet"
        exit 1
    fi
done

if ! command -v g++ >/dev/null 2>&1; then
    echo "g++ not found"
    echo "install a C++ compiler to run this demo"
    exit 1
fi

mkdir -p "$SCRIPT_DIR/logs"
ln -sf "$LIB_SRC" "$LIB_DST"

echo "building C++ example"
g++ -std=c++17 -Wall -Wextra -pedantic -O2 -I"$SCRIPT_DIR/../../liblogjet/include" "$CPP_SRC" -ldl -o "$CPP_BIN"

echo "starting ljd with file-backed OTLP ingest"
"$LJD" --config "$CONFIG" serve &
LJD_PID=$!

cleanup() {
    kill "${LJD_PID:-}" 2>/dev/null || true
}

trap cleanup EXIT INT TERM

sleep 1

echo "sending logs from C++ through liblogjet.so into ljd over OTLP/gRPC"
"$CPP_BIN" "$LIB_DST" "127.0.0.1:4317" 25

sleep 1

echo "opening ljx view on ./logs/cpp-demo.logjet"
"$LJX" view "$SCRIPT_DIR/logs/cpp-demo.logjet"
