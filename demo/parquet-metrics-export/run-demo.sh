#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$SCRIPT_DIR/../../target/debug"
LJD="$TARGET_DIR/ljd"
EMITTER="$TARGET_DIR/metrics-emitter"
LJX="$TARGET_DIR/ljx"
PLUGIN="$TARGET_DIR/libljx_parquet_exporter.so"
CONFIG="$SCRIPT_DIR/logjetd.conf"
OUTPUT_DIR="$SCRIPT_DIR/logs"
OUTPUT_FILE="$OUTPUT_DIR/metrics.logjet"
PARQUET_FILE="$OUTPUT_DIR/metrics.parquet"

for path in "$LJD" "$EMITTER" "$LJX" "$PLUGIN"; do
    if [ ! -x "$path" ]; then
        echo "missing $path"
        echo "build it first with: make demo"
        exit 1
    fi
done

cd "$SCRIPT_DIR"

mkdir -p "$OUTPUT_DIR"
rm -f "$OUTPUT_FILE" "$OUTPUT_DIR/metrics-"*.logjet "$OUTPUT_DIR/metrics.stream-id"

echo "starting ljd with config $CONFIG"
"$LJD" --config "$CONFIG" &
LJD_PID=$!

cleanup() {
    if [ -n "${EMITTER_PID:-}" ]; then
        kill "$EMITTER_PID" 2>/dev/null || true
        wait "$EMITTER_PID" 2>/dev/null || true
    fi
    if [ -n "${LJD_PID:-}" ]; then
        kill "$LJD_PID" 2>/dev/null || true
        wait "$LJD_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT INT TERM

sleep 1

METRIC_COUNT=15
echo "starting metrics-emitter toward 127.0.0.1:4318 ($METRIC_COUNT batches)"
"$EMITTER" 127.0.0.1:4318 "$METRIC_COUNT"

echo "emitter finished; giving ljd time to flush"
sleep 2

echo "stopping ljd"
kill "$LJD_PID" 2>/dev/null || true
wait "$LJD_PID" 2>/dev/null || true
LJD_PID=""

echo "exporting $OUTPUT_FILE to Parquet"
LJX_EXPORTER_PATH="$PLUGIN" "$LJX" --export parquet "$OUTPUT_FILE" -o "$PARQUET_FILE" --force

echo
echo "done: $PARQUET_FILE"
echo
echo "inspect with DuckDB:"
echo "  duckdb -c \"SELECT signal_type, metric_name, metric_type, metric_value_number FROM read_parquet('$PARQUET_FILE') LIMIT 10;\""
