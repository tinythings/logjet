#!/usr/bin/env bash
set -euo pipefail

CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
CARGO_ENV="$CARGO_HOME/env"

if [ -f "$CARGO_ENV" ]; then
    source "$CARGO_ENV" 2>/dev/null || true
fi

have_rustc() { command -v rustc >/dev/null 2>&1; }
have_cargo() { command -v cargo >/dev/null 2>&1; }
have_rustup() { command -v rustup >/dev/null 2>&1; }
have_cc() { command -v cc >/dev/null 2>&1; }

# ── CC / system package helpers ─────────────────────────────────────────────

detect_pkg_manager() {
    case "$(uname -s)" in
        Linux)
            if [ -f /etc/os-release ]; then
                local id
                id=$(awk -F= '/^ID=/{print tolower($2)}' /etc/os-release | tr -d '"')
                case "$id" in
                    ubuntu|debian|pop|linuxmint|elementary|zorin|neon) echo "apt:build-essential" ;;
                    fedora|centos|rhel|rocky|almalinux)               echo "dnf:gcc" ;;
                    arch|manjaro|endeavouros)                         echo "pacman:gcc" ;;
                    opensuse*|suse)                                   echo "zypper:gcc" ;;
                    *) echo "unknown" ;;
                esac
            else
                echo "unknown"
            fi
            ;;
        FreeBSD)  echo "pkg:gcc" ;;
        NetBSD)   echo "pkgin:gcc12" ;;
        *)        echo "unknown" ;;
    esac
}

install_system_cc() {
    local manager="$1"
    local pkg_name="${manager#*:}"
    local cmd="${manager%%:*}"

    case "$cmd" in
        apt)
            echo "Running: sudo apt install -y $pkg_name"
            sudo apt update && sudo apt install -y "$pkg_name"
            ;;
        dnf)
            echo "Running: sudo dnf install -y $pkg_name"
            sudo dnf install -y "$pkg_name"
            ;;
        pacman)
            echo "Running: sudo pacman -S --noconfirm $pkg_name"
            sudo pacman -S --noconfirm "$pkg_name"
            ;;
        zypper)
            echo "Running: sudo zypper install -y $pkg_name"
            sudo zypper install -y "$pkg_name"
            ;;
        pkg)
            echo "Running: sudo pkg install -y $pkg_name"
            sudo pkg install -y "$pkg_name"
            ;;
        pkgin)
            echo "Running: sudo pkgin -y install $pkg_name"
            sudo pkgin -y install "$pkg_name"
            ;;
        *)
            echo "Could not determine how to install a C compiler on this system."
            echo "Please install gcc or clang and re-run this script."
            return 1
            ;;
    esac
}

ensure_cc() {
    if have_cc; then
        return 0
    fi

    echo ""
    echo "A C compiler (cc) is required to build Rust crates."
    echo ""

    local manager
    manager=$(detect_pkg_manager)

    if [ "$manager" = "unknown" ]; then
        echo "Could not detect your package manager."
        echo "Please install gcc or clang, then re-run this script."
        exit 1
    fi

    local pkg_name="${manager#*:}"
    echo "Detected package manager: ${manager%%:*}"
    echo "Required package: $pkg_name"
    echo ""

    read -r -p "Install $pkg_name now? [y/N] " answer
    case "$answer" in
        [yY]|[yY][eE][sS]) ;;
        *)
            echo "Please install gcc or clang, then re-run this script."
            exit 1
            ;;
    esac

    install_system_cc "$manager"

    if ! have_cc; then
        echo "Failed to install C compiler. Please install it manually."
        exit 1
    fi
    echo "C compiler installed: $(cc --version 2>&1 | head -1)"
}

# ── Main setup flow ─────────────────────────────────────────────────────────

if have_rustc && have_cargo && have_rustup; then
    echo "Rust toolchain already installed (rustc $(rustc --version | awk '{print $2}'))."
    if ! cargo clippy --version >/dev/null 2>&1; then
        echo "Installing missing clippy component..."
        rustup component add clippy
    fi
    ensure_cc
    echo ""
    if ! command -v cargo-nextest >/dev/null 2>&1; then
        echo "Installing cargo-nextest..."
        cargo install cargo-nextest --locked
    fi
    echo ""
    echo "Setup complete."
    exit 0
fi
    ensure_cc
    echo ""
    echo "Setup complete."
    exit 0
fi

echo ""
echo "Rust is required to build this project."
echo ""

if have_rustc || have_cargo || have_rustup; then
    echo "A partial Rust installation was detected. Run 'rustup update' and try again."
    exit 1
fi

read -r -p "Install Rust from https://rustup.rs? [y/N] " answer
case "$answer" in
    [yY]|[yY][eE][sS]) ;;
    *)
        echo "Rust not installed. Get it from https://rustup.rs, then re-run this script."
        exit 1
        ;;
esac

echo "Downloading and running rustup-init..."
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable

if [ -f "$CARGO_ENV" ]; then
    source "$CARGO_ENV" 2>/dev/null || true
else
    export PATH="$CARGO_HOME/bin:$PATH"
fi

echo ""
echo "Rust installed. Version info:"
rustc --version
cargo --version
rustup --version

echo ""
echo "Installing clippy..."
rustup component add clippy

ensure_cc

echo ""
echo "Installing cargo-nextest..."
if ! command -v cargo-nextest >/dev/null 2>&1; then
    cargo install cargo-nextest --locked
else
    echo "cargo-nextest already installed."
fi

echo ""
echo "Setup complete."
