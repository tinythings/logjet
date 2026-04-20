#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT_DIR=$(CDPATH= cd -- "$SCRIPT_DIR/../.." && pwd)
GENERATOR="$ROOT_DIR/target/debug/otlp-bofh-logjet-generator"
LJX="$ROOT_DIR/target/debug/ljx"
PLUGIN="$ROOT_DIR/target/debug/libljx_parquet_exporter.so"
OUT_DIR="$SCRIPT_DIR/out"
INPUT="$OUT_DIR/bofh-5000.logjet"
OUTPUT="$OUT_DIR/bofh-5000.parquet"
COUNT="${COUNT:-5000}"

for path in "$GENERATOR" "$LJX" "$PLUGIN"; do
    if [ ! -e "$path" ]; then
        echo "missing $path"
        echo "build first with: cargo build -p otlp-demo --bin otlp-bofh-logjet-generator -p ljx -p ljx-parquet-exporter"
        exit 1
    fi
done

mkdir -p "$OUT_DIR"

echo "generating $COUNT BOFH log records into $INPUT"
"$GENERATOR" "$INPUT" "$COUNT"

echo "exporting $INPUT to $OUTPUT"
LJX_EXPORTER_PATH="$PLUGIN" "$LJX" --export parquet "$INPUT" -o "$OUTPUT" --force

echo
echo "done: $OUTPUT"
