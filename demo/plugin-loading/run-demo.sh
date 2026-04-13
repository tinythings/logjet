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
echo "=== sending 500 syslog messages ==="
echo ""

# 500 messages of varied sizes, single TCP connection.
{
    i=0
    while [ "$i" -lt 500 ]; do
        # Rotate severities and facilities.
        case $((i % 5)) in
            0) pri=14 ;; # user.info
            1) pri=11 ;; # user.err
            2) pri=36 ;; # auth.warning
            3) pri=86 ;; # local2.info
            *) pri=9  ;; # kern.crit
        esac

        # Vary payload size: small / medium / large.
        case $((i % 10)) in
            0)
                # Large (~2KB) — JSON-like blob.
                pad=""
                j=0
                while [ "$j" -lt 40 ]; do
                    pad="${pad}key${j}=value_of_field_${j}_with_some_padding, "
                    j=$((j + 1))
                done
                body="[large-record-${i}] ${pad}"
                ;;
            [1-3])
                # Medium (~200B).
                body="[medium-record-${i}] handler_station_CGStationHandler_updateStation: name=BOFH_FM, freq=107900, ptyCode=10, entryFlags=[20], tokens=[(17, Deutschland)]"
                ;;
            *)
                # Small (~50B).
                body="[rec-${i}] clock skew from hostile NTP daemon"
                ;;
        esac

        printf '<%s>Oct 11 22:14:15 testhost app%s[%s]: %s\n' \
            "$pri" "$((i % 8))" "$((1000 + i))" "$body"
        i=$((i + 1))
    done
} | nc -q0 127.0.0.1 5514

echo ""
echo "=== sent 500 syslog messages ==="
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
