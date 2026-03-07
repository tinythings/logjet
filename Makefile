.PHONY: build dev devel check fix test test-all test-integration setup clean stats arm-devel arm x86-devel x86 setup-arm setup-x86 demo man

DEFAULT_TARGET := build
ARM_TARGET ?= aarch64-unknown-linux-musl
X86_TARGET ?= x86_64-unknown-linux-musl
CORE_WORKSPACE := --workspace --exclude otlp-demo

build: setup
	cargo build $(CORE_WORKSPACE) --release

dev: setup
	cargo build --verbose $(CORE_WORKSPACE)

devel: dev

check: setup
	cargo clippy $(CORE_WORKSPACE) --all-targets --all-features -- -D warnings

fix: setup
	cargo clippy $(CORE_WORKSPACE) --fix --all-targets --all-features --allow-dirty --allow-staged -- -D warnings

test: setup
	@if command -v cargo-nextest >/dev/null 2>&1; then \
		cargo nextest run $(CORE_WORKSPACE); \
	else \
		echo "cargo-nextest not available, falling back to cargo test $(CORE_WORKSPACE)"; \
		cargo test $(CORE_WORKSPACE); \
	fi

test-integration: setup
	@if command -v cargo-nextest >/dev/null 2>&1; then \
		cargo nextest run -p logjetd --test bridge_flows; \
	else \
		echo "cargo-nextest not available, falling back to cargo test -p logjetd --test bridge_flows"; \
		cargo test -p logjetd --test bridge_flows; \
	fi

test-all: test

arm-devel: setup setup-arm
	cargo build $(CORE_WORKSPACE) --target $(ARM_TARGET)

arm: setup setup-arm
	cargo build $(CORE_WORKSPACE) --release --target $(ARM_TARGET)

x86-devel: setup setup-x86
	cargo build $(CORE_WORKSPACE) --target $(X86_TARGET)

x86: setup setup-x86
	cargo build $(CORE_WORKSPACE) --release --target $(X86_TARGET)

demo: devel
	cargo build -p otlp-demo

man:
	@command -v pandoc >/dev/null 2>&1 || { echo "pandoc not found. Install pandoc to build manpages."; exit 1; }
	@mkdir -p doc/manpage
	pandoc --standalone --to man doc/manpage/logjetd.1.md -o doc/manpage/logjetd.1

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
