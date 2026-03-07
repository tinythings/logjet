# File Tooling Demo

This demo shows the operational file-management commands for file mode:

- `logjetd segments`
- `logjetd prune`

## Point

File mode is append-only and rotates by `file.size`.

That is deliberate. It keeps writing simple and predictable. Archive cleanup is
then an explicit operator step:

- inspect the current rotated spool
- preview what would be removed
- prune oldest segments by file count or byte budget

## Build

From the project root:

```bash
make demo
```

## Run

From this directory:

```bash
./run-demo.sh
```

The script:

1. starts file-backed `logjetd`
2. sends enough OTLP batches to rotate several `ops*.logjet` files
3. shows the current segment layout with `logjetd segments`
4. previews pruning by file count
5. previews pruning by byte budget
6. prunes oldest segments and keeps only the newest two files
7. shows the remaining segment layout

## Commands Shown

List the rotated spool:

```bash
../../target/debug/logjetd segments --path ./logs --name ops.logjet
```

Preview pruning by file count:

```bash
../../target/debug/logjetd prune --path ./logs --name ops.logjet --keep-files 2 --dry-run
```

Preview pruning by byte budget:

```bash
../../target/debug/logjetd prune --path ./logs --name ops.logjet --keep-bytes 2048 --dry-run
```

Prune for real:

```bash
../../target/debug/logjetd prune --path ./logs --name ops.logjet --keep-files 2
```

## Expected Result

Before pruning, `segments` prints several `ops*.logjet` files with ordered
segment ids and sequence ranges.

After pruning:

- only the newest two segment files remain
- the active newest segment is still present
- the oldest archived files are gone
