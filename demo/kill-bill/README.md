# Kill Bill Demo

This demo shows the core recovery property of the `.logjet` format:

- a file does not need a single irreplaceable header at byte zero
- each block carries its own sync marker and integrity check
- a reader can skip damaged bytes and recover later good blocks

That is the point where many older header-centric binary formats fail. If the
front of the file is gone, they are finished. `logjet` is not.

## Build First

From the project root:

```bash
make demo
```

## Run

From this directory:

```bash
./run-demo.sh
```

## What It Does

The script does this:

1. starts `ljd` in file mode
2. writes 100 numbered OTLP log messages into one `killbill.logjet` file with a short pacing delay so several physical blocks are created
3. stops `ljd`
4. cuts out the middle third of the file by raw bytes
5. saves only that byte slice as `./damaged/killbill.logjet`
6. inspects the original file
7. inspects the damaged middle-third file
8. replays the damaged file into the mock collector

The damaged file does not start on a real block boundary. Its front is just a
raw byte slice from the middle of the original file.
The pacing delay is intentional: it makes the demo deterministic across fast
machines by ensuring the original file contains several independently
recoverable blocks before the byte cut.

## Expected Result

During `inspect` on the damaged file, you should see:

- some bad blocks or skipped bytes at the start
- then recovered records

During replay, you should see:

- a later subset of the `KILL BILL 001` to `KILL BILL 100` messages
- not the full set
- the recovered messages still in ascending sequence order

That is the actual point:

- the beginning is gone
- the rest is not useless

## Files

- `./logs/killbill.logjet`
  - intact original file
- `./damaged/killbill.logjet`
  - only the middle third of the original file

## Why This Matters

If storage is damaged in the middle, or if only a later byte range of the file
survives, `logjet` can still recover later valid blocks. That is exactly the
sort of corruption-resilience this project is built for.
