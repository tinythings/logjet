# Upstream Reset Resume Demo

This demo shows the missing resume edge case that used to be a problem:

- the consumer side has a saved bridge state file
- the upstream appliance restarts with a fresh in-memory stream
- sequence numbers on the upstream start again from `1`

Without stream identity, the consumer could get stuck forever above the new
upstream sequence range.

With the current implementation:

- the replay listener sends a stream identity hello
- the consumer bridge detects that the upstream stream changed
- the saved sequence is reset automatically
- the new upstream stream is forwarded instead of being ignored forever

## Build First

From the project root:

```bash
make demo
```

## Terminal 1: Start The First Upstream

From this directory:

```bash
./run-appliance-alpha.sh
```

This starts:

1. appliance-side `logjetd`
2. a simple `ALPHA 001`, `ALPHA 002`, `ALPHA 003` message stream

This script also removes any old `bridge.state` file before the first run.

## Terminal 2: Start Consumer

From this directory:

```bash
./run-consumer.sh
```

This starts:

1. the collector
2. consumer-side `logjetd bridge`

The consumer keeps its state in:

```yaml
upstream.state-file: ./bridge.state
```

## Workflow

1. start `./run-appliance-alpha.sh`
2. start `./run-consumer.sh`
3. let a few `ALPHA` messages arrive
4. stop `./run-appliance-alpha.sh`
5. leave the consumer running
6. start `./run-appliance-bravo.sh`

## What You Should See

During the first phase, the collector prints:

```text
ALPHA 001: this is the first upstream stream
ALPHA 002: this is the first upstream stream
```

After you stop the first appliance and start the second one, the collector
should start showing:

```text
BRAVO 001: this is a fresh upstream stream after reset
BRAVO 002: this is a fresh upstream stream after reset
```

That is the point:

- the consumer still has an old saved sequence from `ALPHA`
- the new `BRAVO` stream starts again from low sequence numbers
- bridge detects the changed upstream stream identity
- bridge resets stale saved state automatically
- the new stream is forwarded instead of being skipped

## Notes

- appliance storage mode here is memory only
- the appliance restart creates a fresh upstream stream identity
- the consumer bridge keeps running during the upstream restart
- this demo is about upstream reset handling, not ordinary consumer restart
