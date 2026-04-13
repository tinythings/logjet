#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$SCRIPT_DIR/../../target/debug"
LJD="$TARGET_DIR/ljd"
EMITTER="$TARGET_DIR/otlp-bofh-emitter"
RECORDS=200

if [ ! -x "$LJD" ]; then
    echo "missing $LJD — run: make demo"
    exit 1
fi

if [ ! -x "$EMITTER" ]; then
    echo "missing $EMITTER — run: make demo"
    exit 1
fi

cd "$SCRIPT_DIR"

# Clean previous run
rm -rf bench-data
mkdir -p bench-data/none bench-data/lz4 bench-data/zstd

PIDS=""

cleanup() {
    for pid in $PIDS; do
        kill "$pid" 2>/dev/null || true
    done
    wait 2>/dev/null || true
}

trap cleanup EXIT INT TERM

# Start 3 ljd instances with different codecs
for codec in none lz4 zstd; do
    "$LJD" --config "$SCRIPT_DIR/ljd-${codec}.conf" &
    PIDS="$PIDS $!"
done

sleep 1

echo "Sending $RECORDS OTLP log records to each codec..."
echo ""

# Fire records at each instance (sequential to avoid port races)
for port in 4320 4321 4322; do
    "$EMITTER" "127.0.0.1:${port}" --count "$RECORDS" --interval-ms 0 2>/dev/null
done

# Give flush thread time to write
sleep 1

# Stop ljd instances so blocks flush on shutdown
cleanup
PIDS=""
sleep 1

# Collect stats
NONE_STATS=$("$LJD" inspect "bench-data/none" 2>/dev/null | grep "^stats ")
LZ4_STATS=$("$LJD" inspect "bench-data/lz4" 2>/dev/null | grep "^stats ")
ZSTD_STATS=$("$LJD" inspect "bench-data/zstd" 2>/dev/null | grep "^stats ")

extract() {
    echo "$1" | tr ' ' '\n' | grep "^${2}=" | head -1 | cut -d= -f2
}

NONE_RECORDS=$(extract "$NONE_STATS" "records_ok")
NONE_BLOCKS=$(extract "$NONE_STATS" "blocks_ok")
NONE_COMP=$(extract "$NONE_STATS" "compressed")
NONE_UNCOMP=$(extract "$NONE_STATS" "uncompressed")
NONE_DISK=$(du -sb "bench-data/none" | cut -f1)

LZ4_RECORDS=$(extract "$LZ4_STATS" "records_ok")
LZ4_BLOCKS=$(extract "$LZ4_STATS" "blocks_ok")
LZ4_COMP=$(extract "$LZ4_STATS" "compressed")
LZ4_UNCOMP=$(extract "$LZ4_STATS" "uncompressed")
LZ4_DISK=$(du -sb "bench-data/lz4" | cut -f1)

ZSTD_RECORDS=$(extract "$ZSTD_STATS" "records_ok")
ZSTD_BLOCKS=$(extract "$ZSTD_STATS" "blocks_ok")
ZSTD_COMP=$(extract "$ZSTD_STATS" "compressed")
ZSTD_UNCOMP=$(extract "$ZSTD_STATS" "uncompressed")
ZSTD_DISK=$(du -sb "bench-data/zstd" | cut -f1)

pct() {
    if [ "$2" -gt 0 ]; then
        echo "scale=1; $1 * 100 / $2" | bc
    else
        echo "0"
    fi
}

avg() {
    if [ "$2" -gt 0 ]; then
        echo "scale=1; $1 / $2" | bc
    else
        echo "0"
    fi
}

saved() {
    if [ "$1" -gt 0 ]; then
        DIFF=$(( $1 - $2 ))
        echo "scale=1; $DIFF * 100 / $1" | bc
    else
        echo "0"
    fi
}

human() {
    if [ "$1" -ge 1048576 ]; then
        printf "%.1f MiB" "$(echo "scale=1; $1 / 1048576" | bc)"
    elif [ "$1" -ge 1024 ]; then
        printf "%.1f KiB" "$(echo "scale=1; $1 / 1024" | bc)"
    else
        printf "%s B" "$1"
    fi
}

echo "┌───────────────┬──────────────┬──────────────┬────────────────────┐"
echo "│               │ none         │ lz4          │ zstd               │"
echo "├───────────────┼──────────────┼──────────────┼────────────────────┤"
printf "│ records       │ %12s │ %12s │ %18s │\n" "$NONE_RECORDS" "$LZ4_RECORDS" "$ZSTD_RECORDS"
printf "│ blocks        │ %12s │ %12s │ %18s │\n" "$NONE_BLOCKS" "$LZ4_BLOCKS" "$ZSTD_BLOCKS"
printf "│ rec/block     │ %12s │ %12s │ %18s │\n" \
    "$(avg "$NONE_RECORDS" "$NONE_BLOCKS")" \
    "$(avg "$LZ4_RECORDS" "$LZ4_BLOCKS")" \
    "$(avg "$ZSTD_RECORDS" "$ZSTD_BLOCKS")"
echo "├───────────────┼──────────────┼──────────────┼────────────────────┤"
printf "│ uncompressed  │ %12s │ %12s │ %18s │\n" "$(human "$NONE_UNCOMP")" "$(human "$LZ4_UNCOMP")" "$(human "$ZSTD_UNCOMP")"
printf "│ compressed    │ %12s │ %12s │ %18s │\n" "$(human "$NONE_COMP")" "$(human "$LZ4_COMP")" "$(human "$ZSTD_COMP")"
printf "│ payload saved │ %11s%% │ %11s%% │ %17s%% │\n" \
    "$(saved "$NONE_UNCOMP" "$NONE_COMP")" \
    "$(saved "$LZ4_UNCOMP" "$LZ4_COMP")" \
    "$(saved "$ZSTD_UNCOMP" "$ZSTD_COMP")"
echo "├───────────────┼──────────────┼──────────────┼────────────────────┤"
printf "│ disk size     │ %12s │ %12s │ %18s │\n" "$(human "$NONE_DISK")" "$(human "$LZ4_DISK")" "$(human "$ZSTD_DISK")"
printf "│ disk saved    │ %11s%% │ %11s%% │ %17s%% │\n" \
    "0" \
    "$(saved "$NONE_DISK" "$LZ4_DISK")" \
    "$(saved "$NONE_DISK" "$ZSTD_DISK")"
echo "└───────────────┴──────────────┴──────────────┴────────────────────┘"
echo ""
echo "  Higher 'saved' = better.  rec/block > 1 = batching works."
echo "  'payload saved' = compression ratio on block payloads."
echo "  'disk saved'    = total file size reduction vs uncompressed."
echo "  disk includes block headers + sync markers (fixed overhead)."
