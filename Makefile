.PHONY: build dev check fix test setup clean stats arm-devel arm x86-devel x86 setup-arm setup-x86

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
	@if command -v cargo-nextest >/dev/null 2>&1; then \
		cargo nextest run --workspace; \
	else \
		echo "cargo-nextest not available, falling back to cargo test --workspace"; \
		cargo test --workspace; \
	fi

arm-devel: setup setup-arm
	cargo build --workspace --target $(ARM_TARGET)

arm: setup setup-arm
	cargo build --workspace --release --target $(ARM_TARGET)

x86-devel: setup setup-x86
	cargo build --workspace --target $(X86_TARGET)

x86: setup setup-x86
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
	@echo "Rust toolchain looks ready."

setup-arm:
	@rustup target list --installed | grep -qx "$(ARM_TARGET)" || rustup target add "$(ARM_TARGET)"

setup-x86:
	@rustup target list --installed | grep -qx "$(X86_TARGET)" || rustup target add "$(X86_TARGET)"
