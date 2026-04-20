# Exporter release hardening

This document defines the release and operational policy for `ljx` exporter
plugins.

It covers:

- how exporter shared libraries are packaged and distributed
- what CI must verify before calling the path healthy
- what risks exist when loading third-party native plugins
- what minimal support expectations apply to external exporter authors

## Packaging and distribution model

Exporter plugins are packaged as **separate target-specific native artefacts**.

For the built-in Parquet exporter, the intended release model is:

- `ljx` remains the host binary
- `ljx-parquet-exporter` is shipped as a separate `cdylib` artefact
- host and plugin artefacts use the same project release tag
- each release artefact is target-specific
- the plugin is installed into a directory that `ljx` already knows how to scan

Recommended installed layout:

```text
<prefix>/bin/ljx
<prefix>/lib/logjet/exporters/libljx_parquet_exporter.so
```

Equivalent platform names apply on macOS and Windows.

Why separate artefacts instead of stuffing the plugin into the host binary?

- keeps the stable host/plugin ABI honest
- allows separate build and replacement of plugins
- makes operator-controlled rollout explicit
- avoids pretending plugin compatibility is the same thing as Rust crate linkage

Recommended release packaging rules:

- one plugin artefact per supported target triple
- plugin artefact name stays stable across releases
- release notes must state the expected host version/ABI
- packaged plugins should live in non-user-writable install locations

## CI coverage expectations

Release hardening is not just “cargo build succeeded”.

This repo now expects CI to cover at least:

- plugin compilation as part of normal integration paths
- end-to-end `ljx --export parquet` integration in debug/test builds
- plugin loading in release builds

Current repo hooks:

- `make test-integration`
  - includes `tests/ljx_export.rs`
- `make test-exporter-release-smoke`
  - builds release artefacts and runs a real plugin-loading export smoke test
- `.github/workflows/integration-tests.yml`
  - runs integration coverage through the Make target
- `.github/workflows/it-is-alive.yml`
  - builds release binaries and runs release exporter smoke coverage

## Release smoke expectation

A release-smoke check must prove all of these together:

- release `ljx` binary starts
- release plugin shared library is discovered through the real loader path
- ABI negotiation succeeds
- one real `.logjet` input exports to one real Parquet output

The repo helper is:

```text
make test-exporter-release-smoke
```

That currently reuses `scripts/test-exporter-abi-matrix.sh` with:

- host toolchain = pinned repo toolchain
- plugin toolchain = pinned repo toolchain
- profile = `release`

The smoke helper generates a small non-empty `.logjet` input by default and
asserts that the export path reports a non-zero processed record count.
That avoids false positives where a plugin loads and writes an empty Parquet
file footer but never exports real rows.

## Security implications of third-party shared objects

Loading a third-party exporter plugin means loading **native code into the `ljx`
process**.

Treat that as trusted-code execution, not as a harmless data extension.

Main risks:

- arbitrary code execution with the privileges of the `ljx` process
- filesystem access through the host process account
- network access if the plugin chooses to perform it
- data exfiltration from telemetry or host-local files
- denial of service through crashes, hangs, memory abuse, or excessive output
- ABI-compatible but malicious behaviour that the loader cannot distinguish from
  honest behaviour

Operational guidance:

- load only plugins from trusted build pipelines or trusted vendors
- prefer pinned release artefacts and reproducible hashes
- install plugins in root/admin-controlled directories
- do not include user-writable directories in the plugin search path
- run `ljx` with least privilege when handling untrusted telemetry
- treat `LJX_EXPORTER_PATH` as privileged configuration
- if stronger isolation is required, run export in a container, VM, or other
  sandboxed execution boundary

What the current loader does **not** promise:

- cryptographic plugin signing verification
- sandboxing of plugin code
- syscall filtering
- per-plugin process isolation
- resource quotas beyond what the host process environment imposes

So the security model today is explicit trust, not magical safety.

## Minimal support policy for external exporter authors

The project may support external exporter authors, but the support boundary must
stay narrow and cheap.

Minimum compatibility expectations for an external exporter:

- build as a `cdylib`
- expose `ljx_exporter_descriptor_v1`
- implement ABI v1 exactly
- use only the documented C ABI boundary
- document supported `record_type` and `payload_kind` values
- document supported init options and defaults
- avoid retaining borrowed host pointers after the call returns
- accept that only documented behaviour is stable

Minimal support promise from this repo:

- document the stable ABI and data model
- keep ABI-major breakage explicit
- keep additive ABI evolution tail-only within a minor line
- keep loader diagnostics clear when a plugin is incompatible

What is *not* promised to external authors:

- support for private host internals
- support for reaching into non-ABI Rust modules
- support for undocumented loader quirks as a stable contract
- compatibility with arbitrary libc/target mismatches
- debugging of plugin-specific logic bugs unrelated to the ABI contract

If an external plugin fails, the first support questions should be:

- does it implement the documented ABI exactly?
- does it pass host-side loader checks?
- is it built for the correct target/runtime family?
- does it rely only on documented fields and callbacks?

## Recommended author checklist

Before publishing an exporter plugin:

- run loader compatibility checks against current `ljx`
- run a real export smoke test
- test release artefacts, not just debug artefacts
- test at least one separately built host/plugin pair
- document known limits and failure modes
- keep plugin format names globally unique and lowercase

## Related files

- `doc/parquet/exporters-abi.md`
- `doc/parquet/exporters-data-model.md`
- `doc/parquet/export-parquet.md`
- `scripts/test-exporter-abi-matrix.sh`
- `.github/workflows/integration-tests.yml`
- `.github/workflows/it-is-alive.yml`
