# `ljx`

`ljx` is the offline command-line toolbox for `.logjet` files.

It is separate from `ljd` and must stay separate in purpose:

- `logjet` is the Rust library and file format
- `ljd` is the daemon for ingest, transport, replay, and spool management
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

- top-level query/export mode
- `count`
- `filter`
- `stats`
- `view`
- `split`
- `join`
- `dedup`

Current implementation status for release `0.1`:

- implemented first: `count`
- implemented first: `filter`
- planned after that: `stats`, `split`, `join`

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

## `ljx <input>`

Stream one `.logjet` input as NDJSON to stdout.

This is the fast path for shell pipelines and ad hoc inspection when the TUI is
too heavy and rewriting another `.logjet` file is not what you want.

Examples:

```text
ljx telemetry.logjet
ljx telemetry.logjet -F error -i
ljx telemetry.logjet -F error -e 'customer-123|customer-456' -i
ljx telemetry.logjet --fields body,timestamp,service_name
```

Current behaviour:

- default output format is `ndjson`
- output goes to stdout
- predicates are the same as `count` and `filter`
- `--fields` limits NDJSON keys without changing match semantics

For OTLP log records, NDJSON output includes the core record fields when
present, including:

- `body`
- `timestamp`
- `observed_timestamp`
- `severity_text`
- `severity_number`
- `event_name`
- `trace_id`
- `span_id`
- `flags`
- `scope_name`
- `scope_version`
- flattened resource, scope, and record attributes such as `service_name`

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

Payload matchers are repeatable and combined with logical AND:

- repeated `-F` means every literal must match
- repeated `-e` means every regex must match
- `-F` and `-e` can be mixed, and all matchers must pass

Examples:

```text
ljx telemetry.logjet -F error -F customer-123
ljx telemetry.logjet -e 'timeout|deadline exceeded' -e 'customer-123|customer-456'
ljx telemetry.logjet -F error -e 'panic|fatal' -i
```

Within one `-e`, normal regex alternation still applies, so:

- `-e 'foo|bar'` means one regex matcher that accepts either term
- `-e foo -e bar` means both regexes must match somewhere in the payload

## `ljx stats`

Compute summary information for one file.

Intended summary fields:

- record count
- byte size
- timestamp range
- optional per-type or per-field summaries

## `ljx view`

Browse filtered records in an interactive terminal UI.

Current shape:

- search field at the top, applied with `Enter`
- matching records on the left in a one-line-per-record list
- dynamic details for the selected record on the right
- `Enter` opens a full-record popup, `Esc` closes it
- bounded-memory scan that spools matched records to a temp file instead of loading the whole input

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

## `ljx dedup`

Deduplicate log records by collapsing identical or structurally similar bodies.

Three modes, each building on the previous:

- `exact` -- collapse records with byte-identical bodies within the same bucket.
- `hash2` (default) -- canonicalise bodies (normalise numbers, IDs, paths, timestamps),
  then collapse records sharing the same canonical form.
- `full` -- after hash2, run Drain3 template mining on remaining singletons to catch
  near-duplicates that differ by alphabetic tokens.

Records are partitioned into buckets by `(service.name, severity_number)` before any
dedup. No stage ever merges records across buckets.

Intended examples:

```text
ljx dedup telemetry.logjet -o deduped.logjet
ljx dedup telemetry.logjet -o deduped.logjet --mode=exact
ljx dedup telemetry.logjet -o deduped.logjet --mode=full
ljx dedup telemetry.logjet -o deduped.logjet --bucket-by=scope
ljx dedup telemetry.logjet -o deduped.logjet --mode=full --sim-th=0.8
```

Each output record represents a group of collapsed inputs. The original body from the
first-seen record is preserved. Dedup metadata is added as attributes:

- `dedup.count` -- number of records collapsed into this group
- `dedup.mode` -- which stage produced the group (`exact`, `hash2`, `full/canon`, `full/drain3`)
- `dedup.signature` -- hex hash identifying the group
- `dedup.canonical_body` -- normalised body form (hash2 and full modes)
- `dedup.body_shape` -- detected body type (`json`, `kv`, `prefixed`, `freetext`)
- `dedup.first_seen_ns`, `dedup.last_seen_ns` -- timestamp range of the group
- `dedup.time_span_ms` -- duration the pattern was active
- `dedup.exemplar_trace_ids`, `dedup.exemplar_span_ids` -- up to 3 trace/span IDs for RCA
- `dedup.drain3_template` -- Drain3 template with `<*>` wildcards (full mode only)
- `dedup.drain3_cluster_id` -- Drain3 cluster ID (full mode only)

Non-log records (metrics, traces) pass through unchanged.

Bucket extensions via `--bucket-by`:

- `scope` -- add `instrumentation_scope.name` to the bucket key
- `source_line` -- add `code.filepath` + `code.lineno` to the bucket key

Drain3-specific options (full mode only):

- `--sim-th` -- similarity threshold, 0.0 to 1.0 (default 0.7)
- `--drain-depth` -- prefix tree depth (default 3)
- `--extra-delimiters` -- comma-separated extra token delimiters

Expected properties:

- output is valid `.logjet`
- non-log records preserved in original order
- deterministic for exact and hash2 modes (same input, same output)
- full mode is order-dependent (Drain3 produces different templates for different input orders)

## `ljx --export`

`ljx` can export one `.logjet` file to a non-logjet format through either a
built-in exporter or a discovered exporter plugin.

Current formats:

- built-in: `ndjson`
- plugin: `parquet` when the Parquet exporter `.so` is discoverable

Example:

```text
ljx --export parquet telemetry.logjet -o telemetry.parquet --force
```

Important behaviour:

- export is streaming and preserves input record order
- the host owns the output file and passes bytes through callbacks to the plugin
- if the output file already exists, `--force` is required to overwrite it
- `--fields` is supported only for `ndjson` export and limits the exported JSON keys

Exporter plugin discovery order:

1. entries from `LJX_EXPORTER_PATH`, split like a normal platform path list
2. `./exporters`
3. `<ljx executable directory>/exporters`
4. `<ljx executable directory>/../lib/logjet/exporters`
5. on Unix, `/usr/lib/logjet/exporters`
6. on Unix, `/usr/lib/logjet`

`LJX_EXPORTER_PATH` entries may be directories or direct shared-library paths.
Directory roots are scanned for shared libraries.

## Implementation Notes

The simplest useful internal shape for `ljx` is:

- a thin `clap` CLI layer
- one module per subcommand
- small shared helpers for input/output handling
- small shared predicate parsing for record-aware matching

This keeps the code close to the `logjet` reader and writer APIs and avoids
inventing a second abstraction stack before the command surface is proven.
