.PHONY: build dev check fix test setup

DEFAULT_TARGET := build

build:
	cargo build --release

dev:
	cargo build

check:
	cargo clippy --all-targets --all-features -- -D warnings

fix:
	cargo clippy --fix --all-targets --all-features --allow-dirty --allow-staged -- -D warnings

test:
	cargo nextest run

setup:
	@command -v rustc >/dev/null 2>&1 || { echo "rustc not found. Install Rust from https://rustup.rs"; exit 1; }
	@command -v cargo >/dev/null 2>&1 || { echo "cargo not found. Install Rust from https://rustup.rs"; exit 1; }
	@cargo clippy --version >/dev/null 2>&1 || { echo "clippy is missing. Run: rustup component add clippy"; exit 1; }
	@cargo nextest --version >/dev/null 2>&1 || { echo "cargo-nextest is missing. Run: cargo install cargo-nextest"; exit 1; }
	@echo "Rust toolchain looks ready."
