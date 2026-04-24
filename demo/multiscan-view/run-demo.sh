#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$SCRIPT_DIR/../../target/debug"
GENERATOR="$TARGET_DIR/otlp-random-logjet-generator"
LJX="$TARGET_DIR/ljx"
LOG_ROOT="$SCRIPT_DIR/logs"
DATASET_ORDER="${DATASET_ORDER:-merge-ts}"

if [ ! -x "$GENERATOR" ] || [ ! -x "$LJX" ]; then
    echo "missing demo binaries"
    echo "build them first with: make demo"
    exit 1
fi

case "$DATASET_ORDER" in
    concat|merge-seq|merge-ts) ;;
    *)
        echo "invalid DATASET_ORDER: $DATASET_ORDER"
        echo "expected one of: concat, merge-seq, merge-ts"
        exit 1
        ;;
esac

say_yellow() {
    printf '\033[1;33m%s\033[0m\n' "$1"
}

gen() {
    out="$1"
    count="$2"
    seed="$3"
    mkdir -p "$(dirname "$out")"
    "$GENERATOR" "$out" "$count" "$seed" >/dev/null
}

cd "$SCRIPT_DIR"

say_yellow "Preparing the demo data"
rm -rf "$LOG_ROOT"
mkdir -p "$LOG_ROOT"

gen "$LOG_ROOT/fleet-alpha/single-big.logjet" 900 1101
gen "$LOG_ROOT/fleet-alpha/small_1.logjet" 120 1102
gen "$LOG_ROOT/fleet-alpha/small_2.logjet" 120 1103
gen "$LOG_ROOT/fleet-alpha/small_3.logjet" 120 1104
gen "$LOG_ROOT/fleet-alpha/random_a.logjet" 80 1105
gen "$LOG_ROOT/fleet-alpha/random_b.logjet" 80 1106

gen "$LOG_ROOT/fleet-bravo/single-big.logjet" 850 2201
gen "$LOG_ROOT/fleet-bravo/small_1.logjet" 140 2202
gen "$LOG_ROOT/fleet-bravo/small_2.logjet" 140 2203
gen "$LOG_ROOT/fleet-bravo/small_3.logjet" 140 2204
gen "$LOG_ROOT/fleet-bravo/random_a.logjet" 90 2205
gen "$LOG_ROOT/fleet-bravo/random_b.logjet" 90 2206

gen "$LOG_ROOT/fleet-charlie/single-big.logjet" 780 3301
gen "$LOG_ROOT/fleet-charlie/small_1.logjet" 160 3302
gen "$LOG_ROOT/fleet-charlie/small_2.logjet" 160 3303
gen "$LOG_ROOT/fleet-charlie/small_3.logjet" 160 3304
gen "$LOG_ROOT/fleet-charlie/random_a.logjet" 70 3305
gen "$LOG_ROOT/fleet-charlie/random_b.logjet" 70 3306

gen "$LOG_ROOT/fleet-delta/single-big.logjet" 920 4401
gen "$LOG_ROOT/fleet-delta/small_1.logjet" 110 4402
gen "$LOG_ROOT/fleet-delta/small_2.logjet" 110 4403
gen "$LOG_ROOT/fleet-delta/small_3.logjet" 110 4404
gen "$LOG_ROOT/fleet-delta/random_a.logjet" 95 4405
gen "$LOG_ROOT/fleet-delta/random_b.logjet" 95 4406

echo
echo "prepared demo dataset under $LOG_ROOT"
echo "opening ljx view on $LOG_ROOT with --dataset-order $DATASET_ORDER"
exec "$LJX" view --dataset-order "$DATASET_ORDER" "$LOG_ROOT"
