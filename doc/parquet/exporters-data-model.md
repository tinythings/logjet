# Exporter data model v1

Ticket 4 defines the data contract that `ljx` hands to exporter plugins.

This document sits one level above the raw ABI types in `liblogjet/src/export.rs`.
The ABI says **how** bytes cross the boundary. This document says **what they mean**.

## Core decision

ABI v1 gives plugins **raw records plus a small envelope**, not decoded Rust structs.

Plugins receive:

- `record_type`
- `payload_kind`
- `seq`
- `timestamp_unix_ns`
- `payload`

Plugins do **not** receive:

- Rust-owned decoded OTLP structs
- host-side `serde_json::Value`
- borrowed Rust slices beyond the call
- host-managed schema objects

That choice keeps the ABI stable across toolchains and keeps the host lightweight.

## Record fields handed to plugins

For each call to `write_record`, `ljx` passes one `LjxExportRecordV1`.

### `record_type`

Logical logjet record class:

- `LJX_RECORD_TYPE_LOGS`
- `LJX_RECORD_TYPE_METRICS`
- `LJX_RECORD_TYPE_TRACES`

This lets a plugin reject unsupported telemetry classes before looking inside the payload.

### `payload_kind`

Encoding contract for `payload` bytes.

Currently defined:

- `LJX_PAYLOAD_KIND_OPAQUE`
- `LJX_PAYLOAD_KIND_OTLP_EXPORT_LOGS_REQUEST`

Meaning:

- `OPAQUE` means the plugin must treat the bytes as format-local input with no host guarantee beyond raw bytes
- `OTLP_EXPORT_LOGS_REQUEST` means the bytes are a protobuf-encoded OTLP `ExportLogsServiceRequest`

### `seq`

Original logjet sequence number.

Use cases:

- stable ordering
- deterministic row ids
- better error messages
- resumption/checkpoint designs in future exporters

### `timestamp_unix_ns`

Original top-level logjet timestamp in Unix nanoseconds.

This is always present even when the payload is opaque.
A decoded OTLP log record may carry a more specific inner timestamp, but the outer logjet timestamp remains part of the host contract.

### `payload`

Borrowed payload bytes valid only for the duration of the current FFI call.

Rule:

- if a plugin wants to retain payload bytes after `write_record` returns, it must copy them

## Raw vs decoded records

ABI v1 is deliberately **raw-first**.

Decision:

- plugins see the raw record envelope
- plugins see raw payload bytes
- plugins use `payload_kind` and `record_type` to decide whether and how to decode
- the host does not hand plugins decoded OTLP structs

Why:

- no Rust ABI leakage
- smaller stable boundary
- no duplicated host/plugin schema model
- easier compatibility across different Rust versions
- exporters can choose their own decode strategy and dependencies

Built-in `ndjson` export may keep using host-side decode internally, but that is not part of the plugin ABI contract.

## Streaming lifecycle

Exporter lifecycle in ABI v1 is:

1. host loads descriptor
2. host calls `create(host, init)` once
3. host calls `write_record(ctx, record)` zero or more times
4. host calls `finish(ctx)` once on the success path
5. host calls `free(ctx)` exactly once

### `create`

Input:

- host write/flush callbacks
- init option bag

Output:

- plugin-owned exporter context

Responsibilities:

- parse options
- initialise format writer state
- prepare output metadata
- fail early if the configuration is invalid

### `write_record`

Called once per streamed input record.

Responsibilities:

- validate `record_type` / `payload_kind`
- decode payload if needed
- emit rows/chunks/pages incrementally where possible
- keep only bounded in-memory state

### `finish`

Called once after the last input record on the success path.

Responsibilities:

- flush buffered row groups / footers
- write final metadata
- report final plugin-side failures

`finish` is where structured formats such as Parquet typically close files and emit trailing metadata.

### `free`

Always called to destroy the plugin context.

Responsibilities:

- free plugin-owned memory
- close plugin-owned temporary state
- accept partially initialised contexts safely if possible

### Host callbacks

The plugin writes bytes back through:

- `host.write(user, data, len)`
- optional `host.flush(user)`

This means the host owns the final output target while the plugin owns the format encoder.

## Schema and introspection hooks for structured formats

ABI v1 does **not** define a separate schema callback.

That is intentional.

For v1, structured exporters derive schema from a combination of:

- `descriptor.format_name`
- `descriptor.capabilities`
- init option bag
- `record_type`
- `payload_kind`
- plugin-side decoding of the raw payload bytes

So the schema/introspection hook in v1 is the **init option bag plus payload contract**, not a dedicated function pointer.

### Init option namespaces

`LjxExportInitV1.options` is the extensibility point for schema and writer tuning.

Reserved naming guidance:

- `schema.*` — schema selection and schema strictness
- `output.*` — generic output tuning
- `format.*` — format/plugin-specific settings

Recommended v1 keys for structured exporters:

- `schema.mode`
  - `plugin-default`
  - `strict`
  - `best-effort`
- `schema.name`
  - plugin-defined logical schema name
- `output.compression`
  - plugin-defined codec selection
- `output.row-group-rows`
  - target row-group size for columnar formats

Rules:

- unknown keys must be ignored or rejected with a clear plugin error
- plugins must document the keys they actually support
- the host forwards UTF-8 key/value pairs without interpreting plugin-specific values

Future dedicated schema hooks, if needed, must be added in a tail-additive ABI minor revision.

## Large-file behaviour

ABI v1 is defined for streaming export, not whole-file materialisation.

Required behaviour:

- host reads input sequentially from `LogjetReader`
- host hands one record at a time into `write_record`
- host does not build a full decoded dataset before calling the plugin
- plugin must assume input may be arbitrarily large

Practical consequence for plugin authors:

- keep retained state bounded
- flush writer state incrementally
- for columnar formats, accumulate only bounded row-group/page buffers
- do not retain borrowed payload pointers after the call returns

This keeps the host usable on very large `.logjet` files and makes exporter memory behaviour mostly a plugin concern, not a host bottleneck.

## What v1 does not promise

ABI v1 does not promise:

- host-side decoded OTLP structs
- a unified schema registry
- random access callbacks back into the host
- per-record flush after every write
- resumable export checkpoints

Those can be added later only through additive ABI evolution.

## Authoritative pieces

- ABI layout: `liblogjet/src/export.rs`
- C header: `liblogjet/include/liblogjet_export.h`
- Host runtime: `ljx/src/exporter.rs`
- ABI overview: `doc/parquet/exporters-abi.md`
