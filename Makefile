.PHONY: build dev devel check fix test test-unit test-integration test-abi-matrix test-exporter-release-smoke setup clean stats arm-devel arm x86-devel x86 setup-arm setup-x86 demo man advisory

DEFAULT_TARGET := build
ARM_TARGET ?= aarch64-unknown-linux-musl
X86_TARGET ?= x86_64-unknown-linux-musl
CORE_WORKSPACE := --workspace --exclude otlp-demo
MANPAGE_MD := $(wildcard doc/manpage/*.1.md)
MANPAGE_OUT := $(MANPAGE_MD:.md=)

build: setup
	cargo build $(CORE_WORKSPACE) --release

dev: setup
	cargo build --verbose $(CORE_WORKSPACE)

devel: dev

check: setup
	cargo clippy --workspace --all-targets --all-features -- -D warnings

fix: setup
	cargo clippy --workspace --fix --all-targets --all-features --allow-dirty --allow-staged -- -D warnings

test: setup
	cargo build -p ljd -p ljx -p ljx-parquet-exporter
	cargo build -p otlp-demo --bin otlp-bofh-emitter
	cargo nextest run $(CORE_WORKSPACE)

test-unit: setup
	cargo build -p ljx-parquet-exporter -p lj-perfetto-ingest
	cargo nextest run -p logjet --lib -p ljd --bins -p ljx --bin ljx -p lj-perfetto-ingest

test-integration: setup
	cargo build -p ljd -p ljx -p ljx-parquet-exporter
	cargo build -p otlp-demo --bin otlp-bofh-emitter
	cargo nextest run -p ljd --test bridge_flows
	cargo nextest run -p logjet --test ljx_cli
	cargo nextest run -p logjet --test ljx_export

test-abi-matrix: setup
	bash scripts/test-exporter-abi-matrix.sh

test-exporter-release-smoke: setup
	HOST_TOOLCHAIN=$$(awk -F'"' '/channel = / { print $$2; exit }' rust-toolchain.toml) \
	PLUGIN_TOOLCHAIN=$$(awk -F'"' '/channel = / { print $$2; exit }' rust-toolchain.toml) \
	PROFILE=release \
	bash scripts/test-exporter-abi-matrix.sh

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
	cargo build -p otlp-demo --bin traces-emitter
	cargo build -p otlp-demo --bin traces-grpc-emitter
	cargo build -p otlp-demo --bin multi-signal-emitter
	cargo build -p otlp-demo --bin metrics-grpc-emitter
	cargo build -p lj-syslog-ingest
	cargo build -p lj-logcat-ingest
	cargo build -p lj-stress-ingest
man: $(MANPAGE_OUT)

$(MANPAGE_OUT): doc/manpage/%.1: doc/manpage/%.1.md
	@command -v pandoc >/dev/null 2>&1 || { echo "pandoc not found. Install pandoc to build manpages."; exit 1; }
	@mkdir -p doc/manpage
	pandoc --standalone --to man $< -o $@

advisory:
	@cargo audit --version >/dev/null 2>&1 || { echo "Installing cargo-audit..."; cargo install cargo-audit --locked; }
	@scripts/audit-table.sh

clean:
	cargo clean

stats:
	tokei . --exclude target --exclude .git

setup:
	@bash scripts/setup-rust.sh

setup-arm:
	@rustup target list --installed | grep -qx "$(ARM_TARGET)" || rustup target add "$(ARM_TARGET)"

setup-x86:
	@rustup target list --installed | grep -qx "$(X86_TARGET)" || rustup target add "$(X86_TARGET)"
