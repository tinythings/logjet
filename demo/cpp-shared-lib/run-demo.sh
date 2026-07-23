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
CONFIG_GRPC="$SCRIPT_DIR/ljd.conf"
CONFIG_HTTP="$SCRIPT_DIR/ljd-http.conf"
COUNT="${1:-25}"

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

# Run from the demo dir so ljd resolves the relative file.path (./logs).
cd "$SCRIPT_DIR"

echo "starting ljd: OTLP/gRPC on 127.0.0.1:4317 and OTLP/HTTP on 127.0.0.1:4318"
"$LJD" --config "$CONFIG_GRPC" serve &
LJD_GRPC_PID=$!
"$LJD" --config "$CONFIG_HTTP" serve &
LJD_HTTP_PID=$!

cleanup() {
    kill "${LJD_GRPC_PID:-}" 2>/dev/null || true
    kill "${LJD_HTTP_PID:-}" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

sleep 1

echo
echo "=== OTLP/gRPC: per-connection, reuse, batch, async ==="
"$CPP_BIN" "$LIB_DST" "127.0.0.1:4317" "$COUNT" grpc

echo
echo "=== OTLP/HTTP: per-connection, reuse, batch, async ==="
"$CPP_BIN" "$LIB_DST" "127.0.0.1:4318" "$COUNT" http

sleep 1

echo
echo "results: ./logs/cpp-demo.logjet (gRPC) and ./logs/cpp-demo-http.logjet (HTTP)"
echo "opening ljx view on the gRPC capture (quit to open the HTTP capture)"
"$LJX" view "$SCRIPT_DIR/logs/cpp-demo.logjet"
"$LJX" view "$SCRIPT_DIR/logs/cpp-demo-http.logjet"
