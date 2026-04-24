#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$SCRIPT_DIR/../../target/debug"
LJD="$TARGET_DIR/ljd"
LJX="$TARGET_DIR/ljx"
PLUGIN="$TARGET_DIR/liblj_stress_ingest.so"

for bin in "$LJD" "$LJX" "$PLUGIN"; do
    if [ ! -e "$bin" ]; then
        echo "missing $bin"
        echo "build first with: make demo"
        exit 1
    fi
done

cd "$SCRIPT_DIR"

rm -rf bench-data
mkdir -p bench-data/none bench-data/lz4 bench-data/zstd

PORT_BASE=7100

for codec in none lz4 zstd; do
    PORT=$PORT_BASE
    PORT_BASE=$((PORT_BASE + 2))

    CONFIG=$(mktemp)
    cat > "$CONFIG" <<EOF
output: file
file.path: ./bench-data/$codec
file.size: 5mb
file.name: stress.logjet
file.codec: $codec
ingest.protocol: plugin
ingest.plugin-path: $TARGET_DIR
ingest.use: stress
ingest.listen: 127.0.0.1:$PORT
replay.listen: 127.0.0.1:$((PORT + 1))
EOF

    echo "=== $codec: 25,000 records ==="
    "$LJD" --config "$CONFIG" < /dev/null &
    LJD_PID=$!
    sleep 5
    kill "$LJD_PID" 2>/dev/null || true
    wait "$LJD_PID" 2>/dev/null || true
    rm -f "$CONFIG"

    STATS=$("$LJD" inspect "bench-data/$codec" 2>/dev/null | grep "^stats " || echo "stats records_ok=0 blocks_ok=0")
    echo "  $STATS"
    echo ""
done

echo "┌────────────────────────────────────────────┐"
echo "│  Now open each with ljx and check for      │"
echo "│  'decode failed' on any record.             │"
echo "├────────────────────────────────────────────┤"
echo "│  $LJX view bench-data/none/*.logjet        │"
echo "│  $LJX view bench-data/lz4/*.logjet         │"
echo "│  $LJX view bench-data/zstd/*.logjet        │"
echo "└────────────────────────────────────────────┘"
echo ""
echo "Pick one to open now: [n]one / [l]z4 / [z]std / [q]uit"
read -r choice
case "$choice" in
    n*) exec "$LJX" view bench-data/none/*.logjet ;;
    l*) exec "$LJX" view bench-data/lz4/*.logjet ;;
    z*) exec "$LJX" view bench-data/zstd/*.logjet ;;
    *)  echo "done." ;;
esac
