# C++ Shared Library Demo

This demo shows one C++ process loading a Rust shared library and sending OTLP
logs into `ljd` over both gRPC and HTTP, exercising the per-connection, reuse,
batch, and async send paths.

The path is:

`C++ appliance -> liblogjet.so -> OTLP/gRPC or OTLP/HTTP -> ljd -> .logjet file -> ljx view`

## Build First

From the project root:

```bash
cargo build -p ljd -p ljx -p liblogjet
```

You also need `g++` available locally because the demo compiles the C++
example on demand.

## Run

From this directory:

```bash
./run-demo.sh            # 25 records per phase (default)
./run-demo.sh 100        # custom record count
```

## What It Does

The script:

1. builds the example C++ logger
2. starts two file-backed `ljd` instances: OTLP/gRPC on `127.0.0.1:4317` and OTLP/HTTP on `127.0.0.1:4318`
3. loads `liblogjet.so` through `dlopen`
4. runs the C++ logger once per transport, each exercising four send paths:
   - **per-connection** (`lj_logger_log`)
   - **reuse** (`lj_logger_log_reuse`)
   - **batch** (`lj_logger_log_batch`, many records in one request)
   - **async** (`lj_logger_log_async` with `lj_logger_set_backpressure` + `lj_logger_flush`, then prints the async counters)
5. opens `ljx view` on `./logs/cpp-demo.logjet` (gRPC), then on `./logs/cpp-demo-http.logjet` (HTTP)

## Notes

- the demo runs both OTLP/gRPC and OTLP/HTTP; the C++ source picks the constructor (`lj_logger_new_grpc` / `lj_logger_new_http`) from its 4th argument
- reuse/batch/async work over both transports (HTTP uses a keep-alive connection pool)
- the FFI API is intentionally small: endpoint, service name, timestamp, severity, message body, and string attributes
- those key/value pairs become OTLP `LogRecord.attributes`, which is the standard OTel metadata field for log records
- if the appliance already has JSON metadata, the better long-term shape is to flatten that JSON into separate attributes where possible; a raw JSON blob can still be sent as one string attribute when needed
- the demo uses a local symlink named `liblogjet.so` that points at Cargo's built shared object
