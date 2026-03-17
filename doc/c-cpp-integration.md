# C/C++ Integration

`liblogjet` is a shared library for C and C++ callers that want to emit OTLP
logs into `ljd` without embedding Rust directly in the appliance application.

The ABI is intentionally small:

- create a logger for OTLP/HTTP or OTLP/gRPC
- send one log record at a time
- provide:
  - message body as a string
  - severity
  - timestamp in Unix nanoseconds
  - zero or more string key/value attributes

Those key/value pairs become OTLP `LogRecord.attributes`, which is the standard
OpenTelemetry metadata field for logs.

## Build

From the project root:

```bash
cargo build -p liblogjet
```

Header:

```text
liblogjet/include/liblogjet.h
```

Shared object:

```text
target/debug/libliblogjet.so
```

## Minimal C++ Example

This is the smallest useful flow:

1. load the `.so`
2. resolve the needed symbols
3. create a logger
4. send one warning-level log

```cpp
#include "liblogjet.h"
#include <dlfcn.h>

using new_grpc_fn = lj_logger *(*)(const char *, const char *, uint64_t);
using log_fn = bool (*)(lj_logger *, const lj_log_record *);
using free_fn = void (*)(lj_logger *);
using err_fn = const char *(*)();

int main() {
    void *so = dlopen("./liblogjet.so", RTLD_NOW | RTLD_LOCAL);
    auto lj_logger_new_grpc = reinterpret_cast<new_grpc_fn>(dlsym(so, "lj_logger_new_grpc"));
    auto lj_logger_log = reinterpret_cast<log_fn>(dlsym(so, "lj_logger_log"));
    auto lj_logger_free = reinterpret_cast<free_fn>(dlsym(so, "lj_logger_free"));
    auto lj_error_message = reinterpret_cast<err_fn>(dlsym(so, "lj_error_message"));

    lj_logger *logger = lj_logger_new_grpc("127.0.0.1:4317", "hello-cpp", 2000);
    if (logger == nullptr) return 1;

    const lj_attribute attrs[] = {
        {"tag", "hello-world"},
        {"version", "2.04"},
    };
    const lj_log_record record{
        1700000000000000000ULL,           // timestamp in unix ns
        LJ_SEVERITY_WARN,                 // severity number
        "WARN",                           // severity text
        "Biohazard: C++ is in use!",      // :-)
        attrs,                            // attributes
        sizeof(attrs) / sizeof(attrs[0]), // attributes (len)
    };

    if (!lj_logger_log(logger, &record)) {
        const char *err = lj_error_message();
        (void)err;
    }

    lj_logger_free(logger);
    return 0;
}
```

## Notes

- use `lj_logger_new_http(...)` for OTLP/HTTP
- use `lj_logger_new_grpc(...)` for OTLP/gRPC
- strings must be valid UTF-8
- attribute keys and values are currently string-only by design
- richer C++ usage lives in the demo:
  - [`demo/cpp-shared-lib`](../demo/cpp-shared-lib)
