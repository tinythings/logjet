# `ljx`

`ljx` is the offline command-line toolbox for `.logjet` files.

It is separate from `logjetd` and must stay separate in purpose:

- `logjet` is the Rust library and file format
- `logjetd` is the daemon for ingest, transport, replay, and spool management
- `ljx` is the standalone file tool for inspection and transformation

`ljx` does not control the daemon and does not depend on daemon runtime state.
It operates on `.logjet` files directly.

## Design Goals

The tool is intended to feel closer to `jq`, `parquet-tools`, or plumbing-style
UNIX commands than to a daemon control plane.

Core goals:

- stream records instead of loading entire files into memory
- preserve record ordering
- operate on structured records, not raw bytes
- support stdout where it makes sense
- work cleanly in pipelines
- keep errors direct and actionable

## Current Status

`ljx` is being introduced incrementally.

Documented command set:

- `count`
- `filter`
- `stats`
- `cat`
- `split`
- `join`

Current implementation status for release `0.1`:

- implemented first: `count`
- implemented first: `filter`
- planned after that: `stats`, `cat`, `split`, `join`

The CLI may already expose planned command names, but release `0.1` should only
promise the commands that are actually complete and tested.

## Input and Output Model

`.logjet` files are read using the streaming reader from the `logjet` crate.
That reader is sequential and corruption-tolerant, but the current API expects a
`Read + Seek` source.

That means:

- normal file paths are the primary input mode
- stdout output is straightforward for stream-producing commands
- stdin support needs explicit policy because generic pipes are not seekable

If stdin is unsupported for a given release, `ljx` should fail loudly and say
why. If stdin is later supported by spooling to a temporary file, that behaviour
should be documented as an explicit implementation choice.

## Command Intent

## `ljx count`

Count records in one `.logjet` file, optionally subject to a record-aware
predicate.

Intended examples:

```text
ljx count telemetry.logjet
ljx count telemetry.logjet --type logs
ljx count telemetry.logjet --seq-min 1000 --seq-max 2000
ljx count telemetry.logjet -F error -i
ljx count telemetry.logjet -e 'java\..*\.bs'
```

Expected properties:

- reads sequentially
- preserves file order even though output is only a number
- does not decode or inspect payload schema

## `ljx filter`

Write only matching records to another `.logjet` stream.

Intended examples:

```text
ljx filter telemetry.logjet -o errors.logjet --type logs
ljx filter telemetry.logjet -o - --ts-min 1700000000000000000 > tail.logjet
ljx filter telemetry.logjet -o only-errors.logjet -F error -i
ljx filter telemetry.logjet -o suspect.logjet -e 'java\..*\.bs'
```

Expected properties:

- input order is preserved
- output stays valid `.logjet`
- matching is done per record, not by byte scanning the file

Supported payload matching modes:

- `-F`, `--fixed-string` for literal payload substring matching
- `-e`, `--grep` for grep-style regex matching
- `-i`, `--ignore-case` to make either payload matcher case-insensitive

`ljx` uses one payload matcher at a time. `-F` and `-e` are mutually exclusive.

## `ljx stats`

Compute summary information for one file.

Intended summary fields:

- record count
- byte size
- timestamp range
- optional per-type or per-field summaries

## `ljx cat`

Render records in a human-readable form suitable for terminal inspection.

Open questions:

- whether payload bytes should default to hex, escaped text, or a compact mixed format
- how much payload to print before truncating

## `ljx split`

Split one `.logjet` input into multiple `.logjet` outputs.

Target split modes:

- by record count
- by byte budget
- by timestamp window, when the semantics are nailed down

## `ljx join`

Join multiple `.logjet` segments into one ordered output stream.

Potential validation:

- sequence continuity checks
- timestamp monotonicity checks

## Implementation Notes

The simplest useful internal shape for `ljx` is:

- a thin `clap` CLI layer
- one module per subcommand
- small shared helpers for input/output handling
- small shared predicate parsing for record-aware matching

This keeps the code close to the `logjet` reader and writer APIs and avoids
inventing a second abstraction stack before the command surface is proven.
