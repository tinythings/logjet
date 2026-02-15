#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$SCRIPT_DIR/../../target/debug"
LOGJETD="$TARGET_DIR/logjetd"
EMITTER="$TARGET_DIR/otlp-bofh-emitter"
CONFIG="$SCRIPT_DIR/appliance-logjetd.conf"
STATE_FILE="$SCRIPT_DIR/bridge.state"

for bin in "$LOGJETD" "$EMITTER"; do
    if [ ! -x "$bin" ]; then
        echo "missing $bin"
        echo "build everything first with: make demo"
        exit 1
    fi
done

cd "$SCRIPT_DIR"

rm -f "$STATE_FILE"

echo "starting appliance-side logjetd with config $CONFIG"
"$LOGJETD" --config "$CONFIG" &
LOGJETD_PID=$!

cleanup() {
    kill "${LOGJETD_PID:-}" 2>/dev/null || true
}

trap cleanup EXIT INT TERM

sleep 1

dialogue_line() {
    number="$1"
    index=$(( (number - 1) % 6 ))
    case "$index" in
        0) printf 'DIALOG %03d ALICE: Bob, do you copy?\n' "$number" ;;
        1) printf 'DIALOG %03d BOB: I copy your first message.\n' "$number" ;;
        2) printf 'DIALOG %03d ALICE: Good, I will continue after line %03d.\n' "$number" $((number - 1)) ;;
        3) printf 'DIALOG %03d BOB: I confirm line %03d and wait for the next part.\n' "$number" $((number - 1)) ;;
        4) printf 'DIALOG %03d ALICE: Then remember that line %03d was delivered.\n' "$number" $((number - 1)) ;;
        5) printf 'DIALOG %03d BOB: Confirmed, continue after line %03d.\n' "$number" $((number - 1)) ;;
    esac
}

echo "starting dialogue traffic toward appliance-side logjetd"

seq_no=1
while :; do
    message=$(dialogue_line "$seq_no")
    "$EMITTER" 127.0.0.1:4318 --once --service-name "dialogue-emitter" --message "$message"
    seq_no=$((seq_no + 1))
    sleep 1
done
