# Remote Drain TLS Demo

This demo shows the same network shape as the plain remote-drain demo, but with
TLS enabled on the daemon-to-daemon replay link:

```text
OA -> logjetd <=TLS=> logjetd -> Vector
```

In this demo:

- the appliance-side `logjetd` accepts OTLP/HTTP logs locally
- the appliance-side emitter stands in for `OA`
- the remote-side `logjetd` runs in `bridge` mode
- the replay connection between the two `logjetd` instances is protected with TLS
- the remote-side collector mockup stands in for Vector

This demo exercises all current replay/bridge TLS features:

- `tls.enable`
- `tls.ca-file`
- `tls.cert-file`
- `tls.key-file`
- `tls.require-client-cert`
- `tls.server-name`

Specifically:

- the appliance-side replay listener presents a server certificate
- the remote-side bridge verifies that certificate against the demo CA
- the appliance-side listener requires a client certificate
- the remote-side bridge presents its own client certificate
- the bridge connects to `127.0.0.1`, but validates the server certificate with:
  - `tls.server-name: appliance.demo.logjet`

## Build First

From the project root:

```bash
make demo
```

That gives you:

- `target/debug/logjetd`
- `target/debug/otlp-bofh-emitter`
- `target/debug/otlp-demo-collector`

## Terminal 1: Appliance Side

From this directory:

```bash
./run-appliance.sh
```

This starts:

1. appliance-side `logjetd` with local OTLP/HTTP ingest
2. memory retention with:
   - `buffer.keep: 3`
   - `buffer.messages: 5`
3. 8 manual startup messages:
   - `BOOT MESSAGE #1`
   - ...
   - `BOOT MESSAGE #8`
4. continuous BOFH traffic pointing at the appliance-side daemon

The appliance-side replay listener binds to:

```text
127.0.0.1:7002
```

## Terminal 2: Remote Side

From this directory:

```bash
./run-remote.sh
```

This starts:

1. the colorful OTLP collector mockup on `127.0.0.1:4320`
2. remote-side `logjetd bridge`
3. a TLS connection from the remote side into the appliance replay listener

Expected result:

- `BOOT MESSAGE #1`, `BOOT MESSAGE #2`, and `BOOT MESSAGE #3` always appear
- the other boot messages may or may not appear, depending on when the remote side starts
- backlog already retained on the appliance side is drained first
- then new BOFH messages continue to appear live in the colorful collector

## Certificates

The demo ships a small local CA and two leaf certificates under:

```text
./certs
```

Files:

- `ca.pem`
- `appliance.pem`
- `appliance.key`
- `remote.pem`
- `remote.key`

These are demo credentials only. Do not use them anywhere real.

## Notes

- appliance storage mode here is in-memory buffer mode
- appliance replay listener uses the internal wire protocol inside TLS
- remote `bridge` forwards OTLP logs to `collector.url`
- OTLP ingest and OTLP collector export are still plain transport in this demo
