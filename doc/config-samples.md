# Configuration Samples

## 1. In-Memory Ring Buffer Only

Use this when the appliance should keep a local backlog in RAM only.

```yaml
output: buffer
buffer.size: 100
buffer.preserve: 1000
ingest.listen: 127.0.0.1:7001
replay.listen: 0.0.0.0:7002
replay.poll_ms: 250
```

Behavior:

- keeps roughly 100 KiB of retained records in memory
- preserves the first 1000 retained messages
- drops newer non-preserved backlog first when memory pressure exceeds the configured limit

## 2. File Output with Rotation

Use this when backlog should be emitted into `.logjet` files.

```yaml
output: file
file.path: /var/lib/logjet
file.size: 10240
file.name: vehicle.logjet
ingest.listen: 127.0.0.1:7001
replay.listen: 0.0.0.0:7002
replay.poll_ms: 250
```

Behavior:

- writes to `/var/lib/logjet/vehicle.logjet`
- rotates to `/var/lib/logjet/vehicle-1.logjet`, then `vehicle-2.logjet`, and so on
- each file rotates at about 10 MiB

## 3. Small Lab Setup

Useful for local testing.

```yaml
output: buffer
buffer.size: 32
buffer.preserve: 10
ingest.listen: 127.0.0.1:9001
replay.listen: 127.0.0.1:9002
replay.poll_ms: 100
```

## 4. File-Based Capture on Device

Useful when you want local persistence and later manual extraction.

```yaml
output: file
file.path: /data/logjet
file.size: 2048
file.name: ts.logjet
ingest.listen: 127.0.0.1:7001
replay.listen: 127.0.0.1:7002
replay.poll_ms: 250
```
