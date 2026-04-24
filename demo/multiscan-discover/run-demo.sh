#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$SCRIPT_DIR/../../target/debug"
GENERATOR="$TARGET_DIR/otlp-random-logjet-generator"
LJX="$TARGET_DIR/ljx"
LOG_ROOT="$SCRIPT_DIR/logs"

if [ ! -x "$GENERATOR" ] || [ ! -x "$LJX" ]; then
    echo "missing demo binaries"
    echo "build them first with: make demo"
    exit 1
fi

say_yellow() {
    printf '\033[1;33m%s\033[0m\n' "$1"
}

say_section() {
    printf '\n\033[1;36m%s\033[0m\n' "$1"
}

show_cmd() {
    printf '\033[2m$ %s\033[0m\n' "$*"
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

gen "$LOG_ROOT/fleet-alpha/api.logjet" 180 5101
gen "$LOG_ROOT/fleet-alpha/workers.logjet" 120 5102
gen "$LOG_ROOT/fleet-alpha/errors.logjet" 90 5103

gen "$LOG_ROOT/fleet-bravo/api.logjet" 160 6201
gen "$LOG_ROOT/fleet-bravo/workers.logjet" 140 6202
gen "$LOG_ROOT/fleet-bravo/errors.logjet" 100 6203

gen "$LOG_ROOT/fleet-charlie/api.logjet" 150 7301
gen "$LOG_ROOT/fleet-charlie/workers.logjet" 130 7302
gen "$LOG_ROOT/fleet-charlie/errors.logjet" 110 7303

echo
echo "prepared demo dataset under $LOG_ROOT"

say_section "Full JSON discovery summary"
show_cmd "$LJX discover $LOG_ROOT --type logs --top-services 5"
"$LJX" discover "$LOG_ROOT" --type logs --top-services 5

say_section "Paged JSON discovery summary"
show_cmd "$LJX discover $LOG_ROOT --type logs --offset 2 --limit 4"
"$LJX" discover "$LOG_ROOT" --type logs --offset 2 --limit 4

say_section "NDJSON discovery stream filtered to errors"
show_cmd "$LJX discover $LOG_ROOT --type logs --severity ERROR --limit 5 --ndjson"
"$LJX" discover "$LOG_ROOT" --type logs --severity ERROR --limit 5 --ndjson

say_section "Service-filtered discovery summary"
show_cmd "$LJX discover $LOG_ROOT --type logs --service kill-bill --top-services 3"
"$LJX" discover "$LOG_ROOT" --type logs --service kill-bill --top-services 3
