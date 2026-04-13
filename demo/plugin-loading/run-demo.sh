#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$SCRIPT_DIR/../../target/debug"
LJD="$TARGET_DIR/ljd"
LJX="$TARGET_DIR/ljx"
PLUGIN="$TARGET_DIR/liblj_syslog_ingest.so"
CONFIG="$SCRIPT_DIR/logjetd.conf"

for bin in "$LJD" "$LJX" "$PLUGIN"; do
    if [ ! -e "$bin" ]; then
        echo "missing $bin"
        echo "build first with: make demo"
        exit 1
    fi
done

cd "$SCRIPT_DIR"
mkdir -p logs
rm -f logs/*.logjet

echo "starting ljd with syslog plugin on 127.0.0.1:5514"
"$LJD" --config "$CONFIG" &
LJD_PID=$!

cleanup() {
    kill "$LJD_PID" 2>/dev/null || true
}

trap cleanup EXIT INT TERM

sleep 1

echo ""
echo "=== sending syslog messages ==="
echo ""

# Futurama quotes as syslog messages, various facilities and severities.
send() {
    printf '%s\n' "$1" | nc -q0 127.0.0.1 5514
}

send '<14>Oct 11 22:14:15 planet-express bender: Bite my shiny metal ass'
send '<11>Oct 11 22:14:15 planet-express professor: Good news everyone, the reactor is melting'
send '<13>Oct 11 22:14:15 planet-express fry: Not sure if bug or feature'
send '<12>Oct 11 22:14:15 planet-express leela: This is exactly what I was afraid of — nothing'
send '<10>Oct 11 22:14:15 planet-express zoidberg: My doctorate is in art history'
send '<9>Oct 11 22:14:15 planet-express hermes: That was not in the budget, mon'
send '<14>Oct 11 22:14:15 planet-express bender: Have you ever tried simply turning off the TV and hitting children?'
send '<11>Oct 11 22:14:15 planet-express professor: I dont want to live on this planet anymore'
send '<36>Oct 11 22:14:15 planet-express nibbler[1]: The universe is in jeopardy again'
send '<86>Oct 11 22:14:15 planet-express scruffy[42]: Scruffy believes in this company'
send '<13>Oct 11 22:14:15 planet-express fry: Shut up and take my money'
send '<14>Oct 11 22:14:15 planet-express bender: Im going to build my own theme park with blackjack and hookers'

# Multi-line burst in one connection.
{
    printf '<11>Oct 11 22:14:16 planet-express professor: To shreds you say?\n'
    printf '<11>Oct 11 22:14:16 planet-express professor: Well how is his wife holding up?\n'
    printf '<11>Oct 11 22:14:16 planet-express professor: To shreds you say\n'
    printf '<14>Oct 11 22:14:16 planet-express bender: Cheese it!\n'
} | nc -q0 127.0.0.1 5514

echo ""
echo "=== sent 16 syslog messages ==="
echo ""

sleep 1

# Stop ljd so the file is flushed.
kill "$LJD_PID" 2>/dev/null || true
wait "$LJD_PID" 2>/dev/null || true
trap - EXIT INT TERM

sleep 1

echo "opening viewer on stored records..."
LOGJET_FILE=$(find logs/ -name '*.logjet' -type f | head -1)
if [ -z "$LOGJET_FILE" ]; then
    echo "no .logjet file found in logs/"
    exit 1
fi
exec "$LJX" view "$LOGJET_FILE"
