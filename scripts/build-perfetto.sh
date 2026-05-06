#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
PERFETTO_SRC="${1:-$SCRIPT_DIR/../../perfetto}"

if [ ! -d "$PERFETTO_SRC" ]; then
    echo "Usage: $0 /path/to/perfetto/source"
    echo "  Defaults to ../../perfetto if not specified."
    exit 1
fi

cd "$PERFETTO_SRC"

# ── OS / package detection ───────────────────────────────────────────────────

detect_pkg_manager() {
    if [ -f /etc/os-release ]; then
        local id
        id=$(awk -F= '/^ID=/{print tolower($2)}' /etc/os-release | tr -d '"')
        case "$id" in
            ubuntu|debian|pop|linuxmint|elementary|zorin|neon) echo "apt" ;;
            fedora|centos|rhel|rocky|almalinux)               echo "dnf" ;;
            arch|manjaro|endeavouros)                         echo "pacman" ;;
            opensuse*|suse)                                   echo "zypper" ;;
            *) echo "unknown" ;;
        esac
    else
        echo "unknown"
    fi
}

need() {
    command -v "$1" >/dev/null 2>&1
}

ensure_system_deps() {
    local pm
    pm=$(detect_pkg_manager)
    local missing=""

    # python3-venv is needed on Debian/Ubuntu for ensurepip inside venv.
    if [ "$pm" = "apt" ]; then
        if ! python3 -c 'import ensurepip' 2>/dev/null; then
            echo "Installing python3-venv + python3.12-venv..."
            sudo apt install -y python3-venv python3.12-venv || sudo apt install -y python3-venv
        fi
    fi

    # Also install core deps that might be missing.
    for cmd in git python3 curl tar; do
        if ! need "$cmd"; then
            missing="$missing $cmd"
        fi
    done

    if [ -z "$missing" ]; then
        return
    fi

    echo "Missing system packages:$missing"

    if [ "$pm" = "unknown" ]; then
        echo "Install them manually and re-run this script."
        exit 1
    fi

    case "$pm" in
        apt)
            echo "Running: sudo apt install -y git python3 curl tar"
            sudo apt install -y git python3 curl tar
            ;;
        dnf)
            echo "Running: sudo dnf install -y git python3 curl tar"
            sudo dnf install -y git python3 curl tar
            ;;
        pacman)
            echo "Running: sudo pacman -S --noconfirm git python3 curl tar"
            sudo pacman -S --noconfirm git python3 curl tar
            ;;
        zypper)
            echo "Running: sudo zypper install -y git-core python3 curl tar"
            sudo zypper install -y git-core python3 curl tar
            ;;
    esac
}

ensure_system_deps

# ── Clean stale venv from previous failed runs ───────────────────────────────

if [ -d ".venv" ] && [ ! -f ".venv/bin/activate" ]; then
    echo ""
    echo "Removing broken .venv from previous run..."
    rm -rf .venv
fi

# ── Hermetic toolchain download ───────────────────────────────────────────────

echo ""
echo "Downloading hermetic build dependencies (GN, Ninja, clang, libs)..."
echo "This may take a while on first run."

if [ -x tools/install-build-deps ]; then
    python3 tools/install-build-deps
elif [ -f tools/install-build-deps ]; then
    chmod +x tools/install-build-deps
    python3 tools/install-build-deps
else
    echo "tools/install-build-deps not found. Download it from:"
    echo "  https://raw.githubusercontent.com/google/perfetto/main/tools/install-build-deps"
    exit 1
fi

# ── Build ────────────────────────────────────────────────────────────────────

OUT_DIR="out/linux_release"

echo ""
echo "Generating build config..."

python3 tools/gn gen "$OUT_DIR" --args='
    is_debug = false
    is_clang = true
    use_custom_libcxx = true
    treat_warnings_as_errors = false
'

echo ""
echo "Building trace_processor_shell..."
python3 tools/ninja -C "$OUT_DIR" trace_processor_shell

echo ""
echo "Building traced..."
python3 tools/ninja -C "$OUT_DIR" traced

echo ""
echo "Building traced_probes..."
python3 tools/ninja -C "$OUT_DIR" traced_probes

echo ""
echo "Building tracebox..."
python3 tools/ninja -C "$OUT_DIR" tracebox

echo ""
echo "Perfetto build complete."
echo "Binaries:"
echo "  trace_processor_shell  → $OUT_DIR/trace_processor_shell"
echo "  traced                 → $OUT_DIR/traced"
echo "  traced_probes          → $OUT_DIR/traced_probes"
echo "  tracebox               → $OUT_DIR/tracebox"
echo ""
echo "Add to PATH or set LJD_PERFETTO_TRACE_PROCESSOR=$PWD/$OUT_DIR/trace_processor_shell"
