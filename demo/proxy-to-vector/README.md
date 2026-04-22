# Proxy To Vector Demo

This demo sends OTLP logs through two `ljd` processes and into Vector.

Flow:

```text
OTLP logs -> appliance ljd -> bridge ljd -> Vector -> stdout
```

Directory:

```text
demo/proxy-to-vector
```

## Build First

From the project root:

```bash
make demo
```

## Run Vector

From this directory:

```bash
VECTOR_BIN=/usr/bin/vector ./run-vector.sh
```

`VECTOR_BIN` must be the full absolute path to the Vector binary.

## Run One Proxy Mode

This demo contains three runner scripts:

- `run-vector.sh`
- `run-proxy-http.sh`
- `run-proxy-grpc.sh`

Run Vector in terminal 1, then choose the proxy mode you want in terminal 2.

HTTP mode:

```bash
# terminal 1
VECTOR_BIN=/usr/bin/vector ./run-vector.sh

# terminal 2
./run-proxy-http.sh
```

gRPC mode:

```bash
# terminal 1
VECTOR_BIN=/usr/bin/vector ./run-vector.sh

# terminal 2
./run-proxy-grpc.sh
```

## Featurette: Run Both

You can also run both proxy modes at the same time against the same Vector instance.

That works because:

- Vector listens on both:
  - HTTP `127.0.0.1:4318`
  - gRPC `127.0.0.1:4317`
- the two demo modes use different `ljd` ingest and replay ports
- the two demo modes use separate state and log files

```bash
# terminal 1
VECTOR_BIN=/usr/bin/vector ./run-vector.sh

# terminal 2
./run-proxy-http.sh

# terminal 3
./run-proxy-grpc.sh
```

## Endpoints

HTTP mode:

- appliance-side `ljd` accepts OTLP/HTTP on `127.0.0.1:4319`
- bridge-side `ljd` forwards OTLP/HTTP to Vector on `127.0.0.1:4318`

gRPC mode:

- appliance-side `ljd` accepts OTLP/gRPC on `127.0.0.1:4329`
- bridge-side `ljd` forwards OTLP/gRPC to Vector on `127.0.0.1:4317`

## Local Files

- `bridge-http.state`
- `bridge-grpc.state`
- `appliance-http.log`
- `bridge-http.log`
- `appliance-grpc.log`
- `bridge-grpc.log`

These files are recreated inside this demo directory.
