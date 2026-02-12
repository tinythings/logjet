# Drain Once Demo

This demo shows that `buffer.keep` preserves the first startup messages only
until they are drained once.

After a successful draining pass in `upstream.mode: drain`, those preserved
messages are consumed and do not appear again on the next drain.

## Build First

From the project root:

```bash
make demo
```

## Terminal 1: Appliance Side

From this directory:

```bash
./run-appliance.sh
```

This starts:

1. appliance-side `logjetd`
2. three preserved startup messages:
   - `BOOT MESSAGE #1`
   - `BOOT MESSAGE #2`
   - `BOOT MESSAGE #3`
3. additional startup traffic in the rotating tail
4. continuous BOFH traffic

Let it run for a few seconds before starting the consumer side.

## Terminal 2: Drain Twice

From this directory:

```bash
./run-drain-once.sh
```

This starts:

1. the colourful collector
2. a first bridge pass with `upstream.mode: drain`
3. a second bridge pass, while the appliance keeps running

Expected result:

- on the first pass, the collector shows `BOOT MESSAGE #1`, `#2`, and `#3`
- on the second pass, those preserved startup messages do not appear again
- only newer traffic appears on the second pass

This demonstrates:

- `buffer.keep` keeps the first messages only until a draining pass consumes them
- `upstream.mode: drain` is not permanent retention
- a later drain does not replay already consumed startup messages again
