# Bridge Resume Demo

This demo shows the point of `upstream.state-file`.

The appliance side keeps producing logs. The consumer side can die and come
back. When it returns, `ljd bridge` resumes from the last successfully
forwarded sequence instead of replaying from zero.

That means:

- nothing is lost during a short consumer outage
- messages stay in order
- already forwarded messages are not replayed again

## Why This Matters

The appliance can be effectively read-only.

That is fine here because the persisted resume file is **not** written on the
appliance side. It is written on the downstream consumer side.

The split is:

- appliance-side `ljd`
  - receives OTLP
  - keeps backlog in memory
  - exposes a replay listener
  - does not need to write a bridge checkpoint file

- consumer-side `ljd bridge`
  - connects to the appliance replay listener
  - forwards logs to the collector
  - stores the last forwarded sequence in `upstream.state-file`

So this design fits a read-only appliance much better than a design that needs
local writable checkpoint files on the appliance itself.

## Build First

From the project root:

```bash
make demo
```

## Terminal 1: Start Appliance

From this directory:

```bash
./run-appliance.sh
```

This starts:

1. appliance-side `ljd`
2. a dialogue-style message emitter

The appliance side uses in-memory retention only.

It also removes any old local `bridge.state` file before the demo starts, so
the first consumer run begins from sequence zero.

## Terminal 2: Start Consumer

From this directory:

```bash
./run-consumer.sh
```

This starts:

1. the collector
2. consumer-side `ljd bridge`

The consumer config contains:

```yaml
upstream.replay: 127.0.0.1:7002
upstream.mode: keep
upstream.state-file: ./bridge.state
```

## Workflow

1. start the appliance
2. start the consumer
3. let a few dialogue messages appear
4. stop the consumer with `Ctrl+C`
5. leave the appliance running for a while
6. start the consumer again

## What You Should See

The message text carries a sequence number and depends on the previous line.

Example shape:

```text
DIALOG 001 ALICE: Bob, do you copy?
DIALOG 002 BOB: I copy your first message.
DIALOG 003 ALICE: Good, I will continue after line 002.
```

After you restart the consumer:

- the next line should continue from the last delivered number
- earlier lines should not be replayed again
- the dialogue should still make sense in order

That is the point of the state file:

- it remembers where forwarding reached
- it lives on the consumer side
- it lets the bridge resume after restart

## Notes

- appliance storage mode here is memory only
- consumer resume state is stored in `./bridge.state`
- this demo shows persisted bridge resume, not drain mode
