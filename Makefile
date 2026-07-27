.PHONY: help _release release _dev dev _check check _fix fix _test test _test-unit test-unit _test-integration test-integration _test-abi-matrix test-abi-matrix _test-exp-smoke test-exp-smoke _arm arm _arm-devel arm-devel _x86 x86 _x86-devel x86-devel mxrun-init mxrun mxrun-toggle setup clean stats demo man advisory docs docs-serve docs-clean setup-arm setup-x86

C_GROUP := \033[1;38;5;200m
C_ENTRY := \033[1;38;5;253m
C_DESCR := \033[38;5;43m
C_RESET := \033[0m

MXRUN_BIN ?= mxrun
ARM_TARGET ?= aarch64-unknown-linux-musl
X86_TARGET ?= x86_64-unknown-linux-musl
CORE_WORKSPACE := --workspace --exclude otlp-demo
MANPAGE_MD := $(wildcard doc/manpage/*.1.md)
MANPAGE_OUT := $(MANPAGE_MD:.md=)

.DEFAULT_GOAL := help

help:
	@printf "$(C_GROUP)Usage: make <target>\n\n$(C_RESET)"
	@printf "$(C_GROUP)Build$(C_RESET)\n"
	@printf "  $(C_ENTRY)%-24s$(C_RESET) $(C_DESCR)%s$(C_RESET)\n" "release" "Build release binaries"
	@printf "  $(C_ENTRY)%-24s$(C_RESET) $(C_DESCR)%s$(C_RESET)\n" "dev" "Build debug binaries (verbose)"
	@printf "\n"
	@printf "$(C_GROUP)Code quality$(C_RESET)\n"
	@printf "  $(C_ENTRY)%-24s$(C_RESET) $(C_DESCR)%s$(C_RESET)\n" "check" "Run clippy (strict)"
	@printf "  $(C_ENTRY)%-24s$(C_RESET) $(C_DESCR)%s$(C_RESET)\n" "fix" "Auto-fix clippy warnings"
	@printf "\n"
	@printf "$(C_GROUP)Test$(C_RESET)\n"
	@printf "  $(C_ENTRY)%-24s$(C_RESET) $(C_DESCR)%s$(C_RESET)\n" "test" "Run full test suite"
	@printf "  $(C_ENTRY)%-24s$(C_RESET) $(C_DESCR)%s$(C_RESET)\n" "test-unit" "Run unit tests"
	@printf "  $(C_ENTRY)%-24s$(C_RESET) $(C_DESCR)%s$(C_RESET)\n" "test-integration" "Run integration tests"
	@printf "  $(C_ENTRY)%-24s$(C_RESET) $(C_DESCR)%s$(C_RESET)\n" "test-abi-matrix" "Test exporter ABI compatibility"
	@printf "  $(C_ENTRY)%-24s$(C_RESET) $(C_DESCR)%s$(C_RESET)\n" "test-exp-smoke" "Smoke test exporter plugins"
	@printf "\n"
	@printf "$(C_GROUP)Cross-compile$(C_RESET)\n"
	@printf "  $(C_ENTRY)%-24s$(C_RESET) $(C_DESCR)%s$(C_RESET)\n" "arm" "Build release for arm64-musl"
	@printf "  $(C_ENTRY)%-24s$(C_RESET) $(C_DESCR)%s$(C_RESET)\n" "arm-devel" "Build debug for arm64-musl"
	@printf "  $(C_ENTRY)%-24s$(C_RESET) $(C_DESCR)%s$(C_RESET)\n" "x86" "Build release for x86_64-musl"
	@printf "  $(C_ENTRY)%-24s$(C_RESET) $(C_DESCR)%s$(C_RESET)\n" "x86-devel" "Build debug for x86_64-musl"
	@printf "\n"
	@printf "$(C_GROUP)mxrun$(C_RESET)\n"
	@printf "  $(C_ENTRY)%-24s$(C_RESET) $(C_DESCR)%s$(C_RESET)\n" "mxrun-init" "Initialize mxrun config"
	@printf "  $(C_ENTRY)%-24s$(C_RESET) $(C_DESCR)%s$(C_RESET)\n" "mxrun" "Show mxrun status"
	@printf "  $(C_ENTRY)%-24s$(C_RESET) $(C_DESCR)%s$(C_RESET)\n" "mxrun-toggle" "Toggle local/remote builds"
	@printf "\n"
	@printf "$(C_GROUP)Docs$(C_RESET)\n"
	@printf "  $(C_ENTRY)%-24s$(C_RESET) $(C_DESCR)%s$(C_RESET)\n" "docs" "Build docs site (mkdocs)"
	@printf "  $(C_ENTRY)%-24s$(C_RESET) $(C_DESCR)%s$(C_RESET)\n" "docs-serve" "Serve docs at localhost:8000"
	@printf "  $(C_ENTRY)%-24s$(C_RESET) $(C_DESCR)%s$(C_RESET)\n" "docs-clean" "Remove docs output"
	@printf "  $(C_ENTRY)%-24s$(C_RESET) $(C_DESCR)%s$(C_RESET)\n" "man" "Build manpages"
	@printf "\n"
	@printf "$(C_GROUP)Misc$(C_RESET)\n"
	@printf "  $(C_ENTRY)%-24s$(C_RESET) $(C_DESCR)%s$(C_RESET)\n" "demo" "Build all demo binaries"
	@printf "  $(C_ENTRY)%-24s$(C_RESET) $(C_DESCR)%s$(C_RESET)\n" "advisory" "Run cargo-audit"
	@printf "  $(C_ENTRY)%-24s$(C_RESET) $(C_DESCR)%s$(C_RESET)\n" "clean" "Remove build artifacts"
	@printf "  $(C_ENTRY)%-24s$(C_RESET) $(C_DESCR)%s$(C_RESET)\n" "stats" "Show lines of code"

_release:
	cargo build $(CORE_WORKSPACE) --release

release: setup
	@if [ -n "$$SSH_CONNECTION" ]; then $(MAKE) _release; else scripts/maybe-mxrun.sh release || $(MAKE) _release; fi

_dev:
	cargo build --verbose $(CORE_WORKSPACE)

dev: setup
	@if [ -n "$$SSH_CONNECTION" ]; then $(MAKE) _dev; else scripts/maybe-mxrun.sh dev || $(MAKE) _dev; fi

_check:
	cargo clippy --workspace --all-targets --all-features -- -D warnings -A clippy::needless-update

check: setup
	@if [ -n "$$SSH_CONNECTION" ]; then $(MAKE) _check; else scripts/maybe-mxrun.sh check || $(MAKE) _check; fi

_fix:
	cargo clippy --workspace --fix --all-targets --all-features --allow-dirty --allow-staged -- -D warnings

fix: setup
	@if [ -n "$$SSH_CONNECTION" ]; then $(MAKE) _fix; else scripts/maybe-mxrun.sh fix || $(MAKE) _fix; fi

_test:
	cargo build -p ljd -p ljx -p ljx-parquet-exporter
	cargo build -p otlp-demo --bin otlp-bofh-emitter
	cargo nextest run $(CORE_WORKSPACE)

test: setup
	@if [ -n "$$SSH_CONNECTION" ]; then $(MAKE) _test; else scripts/maybe-mxrun.sh test || $(MAKE) _test; fi

_test-unit:
	cargo build -p ljx-parquet-exporter -p lj-perfetto-ingest
	cargo nextest run -p logjet --lib -p ljd --bins -p ljx --bin ljx -p lj-perfetto-ingest

test-unit: setup
	@if [ -n "$$SSH_CONNECTION" ]; then $(MAKE) _test-unit; else scripts/maybe-mxrun.sh test-unit || $(MAKE) _test-unit; fi

_test-integration:
	cargo build -p ljd -p ljx -p ljx-parquet-exporter
	cargo build -p otlp-demo --bin otlp-bofh-emitter
	cargo nextest run -p ljd --test bridge_flows
	cargo nextest run -p logjet --test ljx_cli
	cargo nextest run -p logjet --test ljx_export

test-integration: setup
	@if [ -n "$$SSH_CONNECTION" ]; then $(MAKE) _test-integration; else scripts/maybe-mxrun.sh test-integration || $(MAKE) _test-integration; fi

_test-abi-matrix:
	bash scripts/test-exporter-abi-matrix.sh

test-abi-matrix: setup
	@if [ -n "$$SSH_CONNECTION" ]; then $(MAKE) _test-abi-matrix; else scripts/maybe-mxrun.sh test-abi-matrix || $(MAKE) _test-abi-matrix; fi

_test-exp-smoke:
	HOST_TOOLCHAIN=$$(awk -F'"' '/channel = / { print $$2; exit }' rust-toolchain.toml) \
	PLUGIN_TOOLCHAIN=$$(awk -F'"' '/channel = / { print $$2; exit }' rust-toolchain.toml) \
	PROFILE=release \
	bash scripts/test-exporter-abi-matrix.sh

test-exp-smoke: setup
	@if [ -n "$$SSH_CONNECTION" ]; then $(MAKE) _test-exp-smoke; else scripts/maybe-mxrun.sh test-exp-smoke || $(MAKE) _test-exp-smoke; fi

_arm:
	cargo build $(CORE_WORKSPACE) --release --target $(ARM_TARGET)

arm: setup setup-arm
	@if [ -n "$$SSH_CONNECTION" ]; then $(MAKE) _arm; else scripts/maybe-mxrun.sh arm || $(MAKE) _arm; fi

_arm-devel:
	cargo build $(CORE_WORKSPACE) --target $(ARM_TARGET)

arm-devel: setup setup-arm
	@if [ -n "$$SSH_CONNECTION" ]; then $(MAKE) _arm-devel; else scripts/maybe-mxrun.sh arm-devel || $(MAKE) _arm-devel; fi

_x86:
	cargo build $(CORE_WORKSPACE) --release --target $(X86_TARGET)

x86: setup setup-x86
	@if [ -n "$$SSH_CONNECTION" ]; then $(MAKE) _x86; else scripts/maybe-mxrun.sh x86 || $(MAKE) _x86; fi

_x86-devel:
	cargo build $(CORE_WORKSPACE) --target $(X86_TARGET)

x86-devel: setup setup-x86
	@if [ -n "$$SSH_CONNECTION" ]; then $(MAKE) _x86-devel; else scripts/maybe-mxrun.sh x86-devel || $(MAKE) _x86-devel; fi

mxrun-init: setup
	@command -v mxrun >/dev/null 2>&1 || { echo "mxrun not installed. Run: make setup" >&2; exit 1; }
	@if [ ! -f mxrun.conf ]; then echo "local" > mxrun.conf; fi
	@printf 'active=yes\n' > .mxrun-env
	@MXRUN_CONFIG=mxrun.conf mxrun init || true

mxrun:
	@if [ -f .mxrun-env ] && grep -q '^active=yes' .mxrun-env; then \
	    echo "mxrun: enabled (distributed builds)"; \
	elif [ -f .mxrun-env ]; then \
	    echo "mxrun: disabled (local builds)"; \
	else \
	    echo "mxrun: unconfigured — run make mxrun-init"; \
	fi

mxrun-toggle:
	@if [ -f .mxrun-env ] && grep -q '^active=yes' .mxrun-env; then \
	    printf 'active=no\n' > .mxrun-env && echo "mxrun: disabled (local builds)"; \
	else \
	    printf 'active=yes\n' > .mxrun-env && echo "mxrun: enabled (distributed builds)"; \
	fi

demo: dev
	cargo build -p otlp-demo
	cargo build -p otlp-demo --bin traces-emitter
	cargo build -p otlp-demo --bin traces-grpc-emitter
	cargo build -p otlp-demo --bin multi-signal-emitter
	cargo build -p otlp-demo --bin metrics-grpc-emitter
	cargo build -p lj-syslog-ingest
	cargo build -p lj-logcat-ingest
	cargo build -p lj-stress-ingest

docs:
	mkdocs build --clean

docs-serve:
	mkdocs serve

docs-clean:
	rm -rf site/

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
