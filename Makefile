.PHONY: build dev check fix test setup clean stats arm-devel arm x86-devel x86

DEFAULT_TARGET := build
ARM_TARGET ?= aarch64-unknown-linux-musl
X86_TARGET ?= x86_64-unknown-linux-musl

build: setup
	cargo build --workspace --release

dev: setup
	cargo build --workspace

check: setup
	cargo clippy --workspace --all-targets --all-features -- -D warnings

fix: setup
	cargo clippy --workspace --fix --all-targets --all-features --allow-dirty --allow-staged -- -D warnings

test: setup
	cargo nextest run --workspace

arm-devel: setup
	cargo build --workspace --target $(ARM_TARGET)

arm: setup
	cargo build --workspace --release --target $(ARM_TARGET)

x86-devel: setup
	cargo build --workspace --target $(X86_TARGET)

x86: setup
	cargo build --workspace --release --target $(X86_TARGET)

clean:
	cargo clean

stats:
	tokei . --exclude target --exclude .git

setup:
	@command -v rustc >/dev/null 2>&1 || { echo "rustc not found. Install Rust from https://rustup.rs"; exit 1; }
	@command -v cargo >/dev/null 2>&1 || { echo "cargo not found. Install Rust from https://rustup.rs"; exit 1; }
	@command -v rustup >/dev/null 2>&1 || { echo "rustup not found. Install Rust from https://rustup.rs"; exit 1; }
	@cargo clippy --version >/dev/null 2>&1 || { echo "clippy is missing. Run: rustup component add clippy"; exit 1; }
	@cargo nextest --version >/dev/null 2>&1 || { echo "cargo-nextest is missing. Run: cargo install cargo-nextest"; exit 1; }
	@rustup target list --installed | grep -qx "$(ARM_TARGET)" || rustup target add "$(ARM_TARGET)"
	@rustup target list --installed | grep -qx "$(X86_TARGET)" || rustup target add "$(X86_TARGET)"
	@echo "Rust toolchain looks ready."
