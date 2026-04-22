#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
CERT_GEN="$SCRIPT_DIR/certs/gen-certs.sh"

if [ -z "${VECTOR_BIN:-}" ]; then
    echo "set VECTOR_BIN to the full Vector binary path first"
    echo "example:"
    echo "  VECTOR_BIN=/usr/bin/vector ./run-vector.sh"
    exit 1
fi

case "$VECTOR_BIN" in
    /*) ;;
    *)
        echo "VECTOR_BIN must be an absolute path"
        echo "example:"
        echo "  VECTOR_BIN=/usr/bin/vector ./run-vector.sh"
        exit 1
        ;;
esac

if [ ! -x "$VECTOR_BIN" ]; then
    echo "missing Vector binary: $VECTOR_BIN"
    exit 1
fi

if [ ! -x "$CERT_GEN" ]; then
    echo "missing cert generator: $CERT_GEN"
    exit 1
fi

"$CERT_GEN"
cd "$SCRIPT_DIR"

exec "$VECTOR_BIN" -c "$SCRIPT_DIR/vector.toml"
