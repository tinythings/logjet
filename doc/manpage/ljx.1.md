% LJX(1)
% Bo Maryniuk
% March 2026

# NAME

ljx - offline toolbox for inspecting and transforming `.logjet` files

# SYNOPSIS

`ljx` `count` *input* [*predicate-options*]

`ljx` `filter` *input* `-o` *output* [*predicate-options*] [`--codec` *codec*] [`--block-target-size` *bytes*]

`ljx` `stats` *input*

`ljx` `cat` *input*

`ljx` `split` *input* *output-prefix*

`ljx` `join` *input*...

# DESCRIPTION

`ljx` is the standalone file tool in the `logjet` ecosystem.

It works directly on `.logjet` files and is intentionally separate from
`logjetd`.

`ljx` does:

- inspect `.logjet` data offline
- stream records sequentially
- preserve record ordering
- transform one `.logjet` stream into another
- fit into ordinary UNIX pipelines

`ljx` does not:

- start or control `logjetd`
- depend on daemon runtime state
- grep raw bytes blindly

Operations are record-aware. Matching and transformation are applied to decoded
records with sequence number, timestamp, record type, and payload bytes.

# COMMANDS

## count

Count records in one `.logjet` file.

Examples:

```text
ljx count telemetry.logjet
ljx count telemetry.logjet --type logs
ljx count telemetry.logjet --seq-min 100 --seq-max 1000
```

`count` is part of the initial `0.1` release scope.

## filter

Write matching records to another `.logjet` stream.

Examples:

```text
ljx filter telemetry.logjet -o only-logs.logjet --type logs
ljx filter telemetry.logjet -o - --ts-min 1700000000000000000 > recent.logjet
```

`filter` is part of the initial `0.1` release scope.

## stats

Compute summary information for one file.

Planned summaries include:

- record count
- byte size
- timestamp range
- optional per-type or field statistics

`stats` is planned but may not be complete in release `0.1`.

## cat

Print records in a human-readable form for inspection.

`cat` is planned but may not be complete in release `0.1`.

## split

Split one `.logjet` file into multiple outputs.

Target split modes include record count, byte budget, and timestamp range.

`split` is planned but may not be complete in release `0.1`.

## join

Join multiple `.logjet` segments into one ordered output stream.

Optional validation may include sequence continuity checks.

`join` is planned but may not be complete in release `0.1`.

# OPTIONS

## `-h`, `--help`

Print usage information.

## `-V`, `--version`

Print version information.

## Predicate options

The initial predicate model is intentionally small.

Expected options:

- `--type` *logs|metrics|traces*
- `--seq-min` *n*
- `--seq-max` *n*
- `--ts-min` *unix-ns*
- `--ts-max` *unix-ns*
- `-e`, `--grep` *pattern*
- `-F`, `--fixed-string` *text*
- `-i`, `--ignore-case`

`-e` and `-F` are mutually exclusive.

## Filter output options

## `-o`, `--output` *path*

Write filtered output to *path*.

Use `-` to write the resulting `.logjet` stream to stdout.

## `--codec` *none|lz4*

Select the output block compression codec.

## `--block-target-size` *bytes*

Target uncompressed payload size per output block.

# USAGE EXAMPLES

## 1. Count all records

```text
ljx count telemetry.logjet
```

Use this for a quick cardinality check without printing records.

## 2. Count only logs

```text
ljx count telemetry.logjet --type logs
```

Use this when one file contains mixed logs, metrics, and traces.

## 2a. Count a case-insensitive fixed string

```text
ljx count telemetry.logjet -F error -i
```

Use this when you just want a plain “contains text” match.

## 3. Count a sequence window

```text
ljx count telemetry.logjet --seq-min 100000 --seq-max 200000
```

Use this to check whether a specific sequence span exists in one file.

## 4. Count a timestamp window

```text
ljx count telemetry.logjet --ts-min 1700000000000000000 --ts-max 1700003600000000000
```

Use this to quantify one incident or replay interval.

## 5. Copy all records to a new file

```text
ljx filter telemetry.logjet -o copy.logjet
```

Use this for a straight record-aware rewrite.

## 5a. Keep records containing a literal string

```text
ljx filter telemetry.logjet -o errors.logjet -F 'java.crap.failed'
```

Use this when literal dots should stay literal dots.

## 6. Keep only one record type

```text
ljx filter telemetry.logjet -o only-traces.logjet --type traces
```

Use this to split one mixed file into per-type files.

## 7. Keep only a sequence range

```text
ljx filter telemetry.logjet -o seq-slice.logjet --seq-min 5000 --seq-max 8000
```

Use this to produce a narrow debugging or replay slice.

## 8. Keep only a timestamp range

```text
ljx filter telemetry.logjet -o one-hour.logjet --ts-min 1700000000000000000 --ts-max 1700003600000000000
```

Use this to extract a specific time window into a new `.logjet` file.

## 9. Stream filtered output to stdout

```text
ljx filter telemetry.logjet -o - --type logs > only-logs.logjet
```

Use this when `ljx` is one stage in a shell pipeline.

## 10. Rewrite with explicit output block settings

```text
ljx filter telemetry.logjet -o compact.logjet --codec lz4 --block-target-size 262144
```

Use this when you want explicit output compression and block sizing.

## 11. Filtering design

Filtering is the most important part of `ljx`.

Payload matching is applied to each record payload, not to raw `.logjet`
container bytes and not to rendered `cat` output.

There are two user-facing modes:

- `-F`, `--fixed-string` for literal substring search
- `-e`, `--grep` for grep-style regex search

Case-insensitive matching is enabled with `-i`, `--ignore-case`.

## 12. Regex payload match: wildcard in the middle

```text
ljx filter telemetry.logjet -o suspect.logjet -e 'java\..*\.bs'
```

Use this when the middle of the payload text varies.

## 13. Regex payload match: case-insensitive

```text
ljx filter telemetry.logjet -o errors.logjet -e 'error|fatal|panic' -i
```

Use this when payload text may contain `ERROR`, `error`, or mixed case variants.

## 14. Count regex matches

```text
ljx count telemetry.logjet -e 'timeout|deadline exceeded' -i
```

Use this when you need cardinality instead of an output file.

## 15. Print records for terminal inspection

```text
ljx cat telemetry.logjet
```

Use this when you want a human-readable record listing.

## 16. Print records with hex payload rendering

```text
ljx cat telemetry.logjet --hex-payload
```

Use this when payload bytes are binary and text rendering is misleading.

## 17. Compute file summary statistics

```text
ljx stats telemetry.logjet
```

Intended output includes record count, byte size, and timestamp range.

## 18. Compute per-type or field statistics

```text
ljx stats telemetry.logjet --field-stats
```

Use this for a quick operational summary before deeper analysis.

## 19. Split by record count

```text
ljx split telemetry.logjet chunk --max-records 100000
```

Use this when large files need to be broken into smaller ordered chunks.

## 20. Split by byte budget

```text
ljx split telemetry.logjet shard --max-bytes 268435456
```

Use this when files must fit under a transfer or storage ceiling.

## 21. Split by timestamp range

```text
ljx split telemetry.logjet hour --timestamp-range 1h
```

This use case needs exact window semantics before it should be treated as stable.

## 22. Join ordered segments

```text
ljx join a.logjet b.logjet c.logjet -o merged.logjet
```

Use this when reconstructing one logical stream from pre-split segments.

## 23. Join and validate continuity

```text
ljx join part-1.logjet part-2.logjet -o merged.logjet --validate-sequence-continuity
```

Use this when gaps or overlaps should be treated as an operator-visible problem.

## 24. Count a window, then extract it

```text
ljx count telemetry.logjet --ts-min 1700000000000000000 --ts-max 1700003600000000000
ljx filter telemetry.logjet -o incident.logjet --ts-min 1700000000000000000 --ts-max 1700003600000000000
```

Use this to measure an interval first and extract it second.

## 25. Recompress through stdout

```text
ljx filter telemetry.logjet -o - --codec lz4 > rewritten.logjet
```

Use this when the shell should control the final destination.

## 26. Produce a small reproduction file

```text
ljx filter telemetry.logjet -o repro.logjet --type logs --seq-min 120000 --seq-max 121000
```

Use this to create a minimal bug-report or regression-test input.

# FILES

`ljx` reads and writes `.logjet` files.

Example names:

- `telemetry.logjet`
- `telemetry-1.logjet`
- `replay.logjet`

# EXIT STATUS

`0`
: success

non-zero
: invalid arguments, read failure, corrupt input handling failure, or write failure

# NOTES

The current `logjet` reader API expects a seekable input source.

That means stdin support must either be:

- unsupported with a clear error, or
- implemented by spooling stdin to a temporary file first

The exact `0.1` behaviour should be documented in the CLI help and release notes.
