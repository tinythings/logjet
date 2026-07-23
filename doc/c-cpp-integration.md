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

## Performance: connection reuse, batching, async

`lj_logger_log` opens a fresh connection per record — simplest and most robust,
but the per-connection handshake dominates at scale. Four additional send paths
eliminate that overhead for both gRPC and HTTP (HTTP uses a keep-alive connection
pool, gRPC caches a multiplexed channel):

| Function | What it does | When to use |
|---|---|---|
| `lj_logger_log` | Fresh connection per record | Low rate, simplest path |
| `lj_logger_log_reuse` | One record over a persistent connection | Moderate rate, replaces `_log` for a speedup |
| `lj_logger_log_batch` | Many records in one request (amortised) | Bulk export, flush loops |
| `lj_logger_log_async` | Non-blocking, hands send to a background runtime | High rate, caller must not block |
| `lj_logger_log_batch_async` | Non-blocking batch send | Bulk export without caller latency |

All `_reuse`, `_batch`, and `_async` paths share the persistent connection — the
first call establishes it (slightly slower), every subsequent call reuses it.

### Error semantics

| Path | Return value `false` means | Network failures |
|---|---|---|
| `lj_logger_log`, `_reuse`, `_batch` | Validation, connection, or HTTP/gRPC error | Returned synchronously via `lj_error_message()` |
| `lj_logger_log_async`, `_batch_async` | Validation error only | Counted later via `lj_logger_async_errors()` |

The async paths never report network failures in-band because the send happens
after the function returns. Check `lj_logger_async_errors` after `lj_logger_flush`
or `lj_logger_free`.

### Thread safety

A single `lj_logger *` may be shared across threads: the underlying gRPC channel
and HTTP connection pool are internally synchronised. The async engine (counters,
backpressure semaphore) is also thread-safe.

### Async backpressure

`lj_logger_log_async` and `lj_logger_log_batch_async` never block the caller.
Outstanding sends are bounded by a backpressure policy set before the first send:

```c
lj_logger_set_backpressure(logger, LJ_BACKPRESSURE_DROP, 1024);  // default
```

| Model | Behaviour |
|---|---|
| `LJ_BACKPRESSURE_UNBOUNDED` | Spawn every send (risk: memory under load) |
| `LJ_BACKPRESSURE_DROP` | Bounded to `capacity`; drop + count when full |
| `LJ_BACKPRESSURE_BLOCK` | Bounded; block the caller until a slot frees |

Drain and observe:

```c
// Block until all in-flight sends finish or 5000 ms elapses
bool drained = lj_logger_flush(logger, 5000);

uint64_t errors  = lj_logger_async_errors(logger);
uint64_t dropped = lj_logger_async_dropped(logger);
uint64_t inflight = lj_logger_async_inflight(logger);
```

`lj_logger_free` also drains in-flight sends before freeing resources.

### Async example (gRPC)

```cpp
#include "liblogjet.h"
#include <dlfcn.h>
#include <cstdio>

int main() {
    void *so = dlopen("./liblogjet.so", RTLD_NOW | RTLD_LOCAL);

    auto new_grpc = (lj_logger *(*)(const char *, const char *, uint64_t))
        dlsym(so, "lj_logger_new_grpc");
    auto log_async = (bool (*)(lj_logger *, const lj_log_record *))
        dlsym(so, "lj_logger_log_async");
    auto set_bp = (bool (*)(lj_logger *, int32_t, size_t))
        dlsym(so, "lj_logger_set_backpressure");
    auto flush = (bool (*)(lj_logger *, uint64_t))
        dlsym(so, "lj_logger_flush");
    auto async_errors = (uint64_t (*)(lj_logger *))
        dlsym(so, "lj_logger_async_errors");
    auto async_dropped = (uint64_t (*)(lj_logger *))
        dlsym(so, "lj_logger_async_dropped");
    auto free_logger = (void (*)(lj_logger *))
        dlsym(so, "lj_logger_free");

    lj_logger *logger = new_grpc("127.0.0.1:4317", "demo-async", 2000);
    set_bp(logger, LJ_BACKPRESSURE_DROP, 256);

    lj_attribute attrs[] = {{"tag", "async-demo"}};
    lj_log_record record{
        0,                     // timestamp (0 = now)
        LJ_SEVERITY_INFO,      // severity
        "INFO",                // severity text
        "async hello",         // body
        attrs,                 // attributes
        1,                     // attributes count
    };

    for (int i = 0; i < 1000; i++) {
        log_async(logger, &record);
    }

    flush(logger, 5000);

    uint64_t errors  = async_errors(logger);
    uint64_t dropped = async_dropped(logger);
    printf("errors=%lu dropped=%lu\n", errors, dropped);

    free_logger(logger);
    return 0;
}
```

### Migration

Replace `lj_logger_log` with `lj_logger_log_reuse` for an immediate speedup with
no change to call semantics. For bulk inserts, switch to `lj_logger_log_batch`.
When caller latency matters, use `lj_logger_log_async` or
`lj_logger_log_batch_async` with backpressure control.

A measured comparison lives in [`demo/benchmark-clib`](../demo/benchmark-clib).
