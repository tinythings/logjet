# C++ Shared Library Demo

This demo shows one C++ process loading a Rust shared library and sending OTLP
logs into `ljd` over gRPC.

The path is:

`C++ appliance -> liblogjet.so -> OTLP/gRPC -> ljd -> .logjet file -> ljx view`

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
./run-demo.sh
```

## What It Does

The script:

1. builds the example C++ logger
2. starts file-backed `ljd` on `127.0.0.1:4317`
3. loads `liblogjet.so` through `dlopen`
4. sends 25 OTLP log records from C++ by default
5. opens `ljx view` on the resulting `./logs/cpp-demo.logjet`

## Notes

- the library now supports both OTLP/HTTP and OTLP/gRPC constructors
- this demo specifically uses OTLP/gRPC
- the FFI API is intentionally small: endpoint, service name, timestamp, severity, message body, and string attributes
- those key/value pairs become OTLP `LogRecord.attributes`, which is the standard OTel metadata field for log records
- if the appliance already has JSON metadata, the better long-term shape is to flatten that JSON into separate attributes where possible; a raw JSON blob can still be sent as one string attribute when needed
- the demo uses a local symlink named `liblogjet.so` that points at Cargo's built shared object
