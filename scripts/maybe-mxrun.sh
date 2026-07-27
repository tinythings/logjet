#!/usr/bin/env sh
set -eu

ENTRY="${1:-}"

# mxrun blanks MXRUN_LOCAL_MAKE when calling back — run local build.
[ "${MXRUN_LOCAL_MAKE+set}" = "set" ] && exit 1

ACTIVE=no
[ -f .mxrun-env ] && ACTIVE=$(awk -F= '/^active=/ {print $2}' .mxrun-env 2>/dev/null)

if [ "$ACTIVE" = "yes" ] && [ -f mxrun.conf ]; then
    command -v mxrun >/dev/null 2>&1 || { echo "mxrun not installed. Run: cargo install mxrun" >&2; exit 1; }
    MXRUN_CONFIG=mxrun.conf mxrun run "$ENTRY" || true
    exit 0
fi

exit 1
