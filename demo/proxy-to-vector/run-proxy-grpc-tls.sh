#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT_DIR=$(CDPATH= cd -- "$SCRIPT_DIR/../.." && pwd)
TARGET_DIR="$ROOT_DIR/target/debug"
LJD="$TARGET_DIR/ljd"
EMITTER="$TARGET_DIR/otlp-bofh-grpc-emitter"
CERTS_DIR="$SCRIPT_DIR/certs"
CERT_GEN="$CERTS_DIR/gen-certs.sh"
APPLIANCE_CONFIG="$SCRIPT_DIR/appliance-grpc-tls-logjetd.conf"
BRIDGE_CONFIG="$SCRIPT_DIR/bridge-grpc-tls-logjetd.conf"
APPLIANCE_LOG="$SCRIPT_DIR/appliance-grpc-tls.log"
BRIDGE_LOG="$SCRIPT_DIR/bridge-grpc-tls.log"

for bin in "$LJD" "$EMITTER"; do
    if [ ! -x "$bin" ]; then
        echo "missing $bin"
        echo "build demo bits first with: make demo"
        exit 1
    fi
done

if [ ! -x "$CERT_GEN" ]; then
    echo "missing cert generator: $CERT_GEN"
    exit 1
fi

"$CERT_GEN"

cd "$SCRIPT_DIR"
rm -f bridge-grpc-tls.state "$APPLIANCE_LOG" "$BRIDGE_LOG"

cleanup() {
    kill "${EMITTER_PID:-}" 2>/dev/null || true
    kill "${BRIDGE_PID:-}" 2>/dev/null || true
    kill "${APPLIANCE_PID:-}" 2>/dev/null || true
}

trap cleanup EXIT INT TERM

echo "starting gRPC TLS appliance ljd on 127.0.0.1:4339"
"$LJD" --config "$APPLIANCE_CONFIG" >"$APPLIANCE_LOG" 2>&1 &
APPLIANCE_PID=$!

sleep 1

echo "starting gRPC TLS bridge ljd toward Vector on 127.0.0.1:4417"
"$LJD" --config "$BRIDGE_CONFIG" bridge >"$BRIDGE_LOG" 2>&1 &
BRIDGE_PID=$!

sleep 1

echo "gRPC TLS proxy logs:"
echo "  appliance: $APPLIANCE_LOG"
echo "  bridge:    $BRIDGE_LOG"
echo "sending OTLP gRPC logs into appliance ljd on 127.0.0.1:4339"
"$EMITTER" 127.0.0.1:4339 &
EMITTER_PID=$!

wait "$EMITTER_PID"
