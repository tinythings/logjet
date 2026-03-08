# Secure Pipeline Demo

This demo shows secure OTLP features in one path:

```text
OA -- HTTPS OTLP --> logjetd -- HTTPS OTLP --> collector
```

It demonstrates:

- HTTPS OTLP/HTTP ingest with `ingest.tls-enable`
- HTTPS collector export with `collector.url: https://...`
- collector CA verification with `collector.ca-file`
- collector server-name override with `collector.server-name`

This demo does not use the replay/bridge TLS path. That already has its own demo
under [`../remote-drain-tls`](../remote-drain-tls).

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

The script starts:

1. a demo HTTPS collector on `127.0.0.1:4321`
2. `logjetd` with:
   - HTTPS OTLP ingest on `127.0.0.1:4319`
   - HTTPS collector export to the collector
3. removes old generated `secure*.logjet` demo files from `./logs`
4. the BOFH emitter, sending OTLP/HTTP over HTTPS into `logjetd`
5. `logjetd replay` blasting the stored `.logjet` file into the HTTPS collector

Expected result:

- the emitter prints plain BOFH log batches it sends over HTTPS
- `logjetd` stores them in `./logs`
- the collector prints the same 5 records in colour after replay

## Certificates

This demo uses local demo-only certificates under:

```text
./certs
```

Files:

- `ca.pem`
- `ingest.pem`
- `ingest.key`
- `collector.pem`
- `collector.key`

Do not use these credentials anywhere real.
