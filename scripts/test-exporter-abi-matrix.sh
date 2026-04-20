#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
DEFAULT_TOOLCHAIN=$(awk -F'"' '/channel = / { print $2; exit }' "$ROOT_DIR/rust-toolchain.toml")
HOST_TOOLCHAIN=${HOST_TOOLCHAIN:-$DEFAULT_TOOLCHAIN}
PLUGIN_TOOLCHAIN=${PLUGIN_TOOLCHAIN:-stable}
PROFILE=${PROFILE:-debug}
INPUT=${INPUT:-$ROOT_DIR/logs/cpp-demo.logjet}

sanitize() {
    printf '%s' "$1" | tr -c '[:alnum:]._-' '_'
}

host_tag=$(sanitize "$HOST_TOOLCHAIN")
plugin_tag=$(sanitize "$PLUGIN_TOOLCHAIN")
profile_tag=$(sanitize "$PROFILE")
host_target_dir=${HOST_TARGET_DIR:-$ROOT_DIR/target/abi-matrix/$profile_tag-host-$host_tag}
plugin_target_dir=${PLUGIN_TARGET_DIR:-$ROOT_DIR/target/abi-matrix/$profile_tag-plugin-$plugin_tag}
run_dir=${RUN_DIR:-$ROOT_DIR/target/abi-matrix/run-$profile_tag-host-$host_tag-plugin-$plugin_tag}

ensure_toolchain() {
    local tc=$1
    if ! rustup run "$tc" rustc --version >/dev/null 2>&1; then
        echo "Missing rustup toolchain '$tc'. Install it with: rustup toolchain install $tc" >&2
        exit 1
    fi
}

shared_library_name() {
    case "$(uname -s)" in
        Linux) printf 'libljx_parquet_exporter.so' ;;
        Darwin) printf 'libljx_parquet_exporter.dylib' ;;
        MINGW*|MSYS*|CYGWIN*) printf 'ljx_parquet_exporter.dll' ;;
        *)
            echo "Unsupported host OS for this smoke script: $(uname -s)" >&2
            exit 1
            ;;
    esac
}

ensure_toolchain "$HOST_TOOLCHAIN"
ensure_toolchain "$PLUGIN_TOOLCHAIN"

build_args=()
profile_dir=$PROFILE
if [[ "$PROFILE" != "debug" ]]; then
    build_args+=(--profile "$PROFILE")
fi

mkdir -p "$run_dir"
output="$run_dir/cpp-demo.parquet"
rm -f "$output"

pushd "$ROOT_DIR" >/dev/null

echo "[abi-matrix] building ljx with toolchain $HOST_TOOLCHAIN profile=$PROFILE"
cargo +"$HOST_TOOLCHAIN" build -p ljx --target-dir "$host_target_dir" "${build_args[@]}"

echo "[abi-matrix] building ljx-parquet-exporter with toolchain $PLUGIN_TOOLCHAIN profile=$PROFILE"
cargo +"$PLUGIN_TOOLCHAIN" build -p ljx-parquet-exporter --target-dir "$plugin_target_dir" "${build_args[@]}"

host_bin="$host_target_dir/$profile_dir/ljx"
plugin_so="$plugin_target_dir/$profile_dir/$(shared_library_name)"

if [[ ! -x "$host_bin" ]]; then
    echo "Host binary missing: $host_bin" >&2
    exit 1
fi
if [[ ! -f "$plugin_so" ]]; then
    echo "Plugin library missing: $plugin_so" >&2
    exit 1
fi
if [[ ! -f "$INPUT" ]]; then
    echo "Input file missing: $INPUT" >&2
    exit 1
fi

echo "[abi-matrix] running host=$HOST_TOOLCHAIN plugin=$PLUGIN_TOOLCHAIN"
LJX_EXPORTER_PATH="$plugin_so" "$host_bin" --export parquet "$INPUT" -o "$output" --force

if [[ ! -s "$output" ]]; then
    echo "Expected non-empty Parquet output at $output" >&2
    exit 1
fi

echo "[abi-matrix] success -> $output"

popd >/dev/null
