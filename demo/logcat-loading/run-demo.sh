#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$SCRIPT_DIR/../../target/debug"
LJD="$TARGET_DIR/ljd"
LJX="$TARGET_DIR/ljx"
PLUGIN="$TARGET_DIR/liblj_logcat_ingest.so"
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

echo "feeding fake logcat into ljd via active plugin..."

# Simulate adb logcat output — BOFH excuses from the device.
{
    printf '06-11 08:00:01.001  1000  1000 I SysAdmin : clock skew from a hostile NTP daemon\n'
    printf '06-11 08:00:01.042  1001  1001 W NetStack : magnetic interference from a mislabeled coffee mug\n'
    printf '06-11 08:00:01.100  1002  1002 I Kernel   : temporary routing loop caused by an intern with initiative\n'
    printf '06-11 08:00:01.150  1003  1003 D HwManager: cosmic rays flipped the wrong bit again\n'
    printf '06-11 08:00:01.200  1004  1004 E FanCtrl  : the backup fan controller achieved sentience and quit\n'
    printf '06-11 08:00:01.250  1005  1005 I BootLoader: kernel panic triggered by excessive optimism\n'
    printf '06-11 08:00:01.300  1000  1010 W DbEngine : database latency due to an emotionally unavailable SAN\n'
    printf '06-11 08:00:01.350  1001  1011 F Consultant: a consultant enabled enterprise mode\n'
    printf '06-11 08:00:01.400  1006  1006 I Scheduler: heat death postponed packet delivery\n'
    printf '06-11 08:00:01.450  1007  1007 D PacketMgr: the packet inspector was inspected and found wanting\n'
    printf '06-11 08:00:01.500  1002  1012 I Firmware : firmware entered a spiritual debugging journey\n'
    printf '06-11 08:00:01.550  1000  1013 E SysAdmin : someone rebooted the wrong reality\n'
    printf 'I/SysAdmin(1000): the DNS server has achieved enlightenment and refuses to resolve\n'
    printf 'W/NetStack(1001): packets are being routed through a parallel universe\n'
    printf 'I/HwManager(1003): the DIMM sticks are staging a union walkout\n'
    printf 'E/FanCtrl(1004): thermal paste has gone on a spiritual retreat\n'
} | timeout 3 "$LJD" --config "$CONFIG" || true

echo ""
echo "=== captured 16 logcat messages ==="
echo ""

sleep 1

echo "opening viewer on stored records..."
LOGJET_FILE=$(find logs/ -name '*.logjet' -type f | head -1)
if [ -z "$LOGJET_FILE" ]; then
    echo "no .logjet file found in logs/"
    exit 1
fi
exec "$LJX" view "$LOGJET_FILE"
