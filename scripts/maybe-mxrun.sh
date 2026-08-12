#!/usr/bin/env sh
set -eu

ENTRY="${1:-}"

# mxrun blanks MXRUN_LOCAL_MAKE when calling back — run local build.
[ "${MXRUN_LOCAL_MAKE+set}" = "set" ] && exit 1

ACTIVE=no
[ -f .mxrun-env ] && ACTIVE=$(awk -F= '/^active=/ {print $2}' .mxrun-env 2>/dev/null)

if [ "$ACTIVE" = "yes" ] && [ -f mxrun.conf ]; then
    command -v mxrun >/dev/null 2>&1 || { echo "mxrun not installed. Run: cargo install mxrun" >&2; exit 1; }
    case "$ENTRY" in
        release)    LABEL="Release Build" ;;
        dev)        LABEL="Development Build" ;;
        check)      LABEL="Clippy Check" ;;
        fix)        LABEL="Clippy Fix" ;;
        test)       LABEL="Full Test Suite" ;;
        test-unit)  LABEL="Unit Tests" ;;
        test-integration) LABEL="Integration Tests" ;;
        test-abi-matrix)  LABEL="ABI Matrix Tests" ;;
        test-exp-smoke)   LABEL="Exporter Smoke Tests" ;;
        arm)        LABEL="ARM Release Build" ;;
        arm-devel)  LABEL="ARM Development Build" ;;
        x86)        LABEL="x86 Release Build" ;;
        x86-devel)  LABEL="x86 Development Build" ;;
        *)          LABEL="$ENTRY" ;;
    esac
    MXRUN_CONFIG=mxrun.conf mxrun run --label="$LABEL" "$ENTRY" || true
    exit 0
fi

exit 1
