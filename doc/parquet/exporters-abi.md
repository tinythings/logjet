# Exporter ABI v1

Ticket 1 defines the first stable ABI for `ljx` exporter plugins.

## Goals

- shared libraries built as `cdylib`
- no Rust ABI across the host/plugin boundary
- versioned metadata discovery
- explicit ownership and error rules
- safe evolution across separately compiled Rust versions

## Discovery

Every exporter plugin exports one symbol:

- `ljx_exporter_descriptor_v1`

That symbol returns a pointer to a static `ljx_exporter_descriptor_v1`.

The descriptor advertises:

- `abi_major`
- `abi_minor`
- `plugin_api_version`
- `capabilities`
- `format_name`
- `display_name`
- `default_extension`
- function pointers for create / write_record / finish / last_error / free

## Host plugin discovery order

`ljx` loads exporter plugins in this order:

1. entries from `LJX_EXPORTER_PATH`
2. `./exporters`
3. `<ljx-exe-dir>/exporters`
4. `<ljx-exe-dir>/../lib/logjet/exporters`

Notes:

- `LJX_EXPORTER_PATH` uses the platform path-list separator
- each entry may be either a shared-library file or a directory
- directory contents are scanned in lexical order
- built-in formats win over plugins
- for duplicate plugin formats, first discovered wins and later duplicates are ignored with diagnostics

## Boundary shape

The ABI is C-only:

- `#[repr(C)]` structs only
- primitive integers only
- string views are `(ptr, len)`
- byte views are `(ptr, len)`
- opaque plugin context pointer
- host write/flush callbacks supplied explicitly

No Rust types cross the ABI:

- no `String`
- no `Vec`
- no slices
- no trait objects
- no Rust enums
- no panics across FFI

## Current record contract

ABI v1 pushes raw logjet records into the exporter:

- `record_type`
- `seq`
- `timestamp_unix_ns`
- `payload_kind`
- `payload` bytes

For log exports, the important initial payload kind is:

- `LJX_PAYLOAD_KIND_OTLP_EXPORT_LOGS_REQUEST`

That keeps the ABI small and stable while still allowing structured exporters
such as Parquet plugins to decode OTLP protobuf payloads themselves.

The higher-level meaning of those fields, lifecycle rules, schema option hooks,
and streaming expectations are documented in:

- `doc/parquet/exporters-data-model.md`

## Ownership rules

- descriptor strings must point to static plugin-owned storage
- host init/options/record payload pointers are borrowed only for the call
- plugins must not free or retain host-owned pointers after the call returns
- `last_error` returns a borrowed string view valid until the next call on the
  same exporter context
- `free(NULL)` must be accepted

## Compatibility policy

- `abi_major` change = breaking
- `abi_minor` change = additive only
- all top-level ABI structs carry `struct_size`
- new minor versions may append fields only at the tail
- reserved fields must be zero in v1
- host should reject plugins with mismatched major versions
- host may accept plugins whose minor version is less than or equal to the
  host-supported minor version

## Cross-version Rust compatibility strategy

The compatibility story is intentionally boring:

- the host/plugin boundary is plain C ABI only
- Rust compiler version is therefore *not* part of the runtime contract
- host and plugin may be built separately, at different times, with different
  stable Rust toolchains, as long as they both implement ABI v1 correctly

That works because the boundary never exposes Rust-owned layout-sensitive types.

Validation layers in this repo are:

- authoritative Rust ABI definitions in `liblogjet/src/export.rs`
- matching C header in `liblogjet/include/liblogjet_export.h`
- unit tests in `liblogjet/src/export_ut.rs` for size/default/tail-additive
  invariants
- host-side loader validation in `ljx/src/exporter.rs`
- a separate-toolchain smoke script in `scripts/test-exporter-abi-matrix.sh`

## Supported compatibility matrix

What is intended to work:

| host build | plugin build | support |
|---|---|---|
| same stable Rust toolchain | same stable Rust toolchain | yes |
| different stable Rust toolchains | different stable Rust toolchains | yes, if ABI v1 matches |
| toolchains built at different times from same source tree | separately compiled | yes |

Constraints that still apply:

- same OS ABI family at runtime (`.so`/`.dylib`/`.dll` must match host platform)
- same CPU architecture
- same libc/runtime family where relevant (for example glibc vs musl is not a
  promised interchangeable plugin boundary)
- plugin must be built as a `cdylib`
- plugin must expose `ljx_exporter_descriptor_v1`
- plugin `abi_major` must match host `LJX_EXPORTER_ABI_MAJOR`
- plugin `abi_minor` must be less than or equal to host
  `LJX_EXPORTER_ABI_MINOR`

Non-goals for ABI v1:

- no promise across arbitrary target triples
- no promise across incompatible libc families
- no promise for plugins that smuggle Rust-owned layout-sensitive types behind
  the defined ABI

## Incompatible plugin fallback behaviour

Fallback behaviour is intentionally explicit.

If a plugin is incompatible during discovery:

- the host ignores that plugin
- the loader records a diagnostic describing why
- later compatible plugins for other formats may still load normally

If the user explicitly requests a format that is available only through an
incompatible plugin:

- `ljx` fails with an unknown-format style error
- the error includes searched paths and loader notes

If a plugin loads successfully but fails later in `create`, `write_record`, or
`finish`:

- the export command fails clearly
- plugin-side error text is surfaced where available
- partial output may already exist; no atomic replacement is promised

## Separate-toolchain smoke test

Use this repo helper to validate a host/plugin pair built by different Rust
toolchains:

```text
bash scripts/test-exporter-abi-matrix.sh
```

Useful overrides:

- `HOST_TOOLCHAIN=1.94.0`
- `PLUGIN_TOOLCHAIN=stable`
- `INPUT=/path/to/input.logjet`

That script builds `ljx` and `ljx-parquet-exporter` into separate target
directories and then runs a real `ljx --export parquet ...` smoke test using
the host binary plus the separately built plugin shared library.

By default it generates a small non-empty `.logjet` smoke input first, so the
check fails if plugin loading succeeds but zero records are actually exported.

## Files

Authoritative definitions now live in:

- `liblogjet/src/export.rs`
- `liblogjet/include/liblogjet_export.h`
- `doc/parquet/exporters-data-model.md`
- `doc/parquet/exporter-release.md`
