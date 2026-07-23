#ifndef LIBLOGJET_H
#define LIBLOGJET_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define LJ_SEVERITY_TRACE 1
#define LJ_SEVERITY_DEBUG 5
#define LJ_SEVERITY_INFO 9
#define LJ_SEVERITY_WARN 13
#define LJ_SEVERITY_ERROR 17
#define LJ_SEVERITY_FATAL 21

#define LJ_ATTR_STRING 0
#define LJ_ATTR_INT 1
#define LJ_ATTR_ARRAY 2

// Async backpressure models (lj_logger_set_backpressure).
#define LJ_BACKPRESSURE_UNBOUNDED 0
#define LJ_BACKPRESSURE_DROP 1
#define LJ_BACKPRESSURE_BLOCK 2

// Ingest plugin signal bitmask (in descriptor reserved[0], ABI >= 1.1).
#define LJ_INGEST_SIGNAL_LOGS    (1u << 0)
#define LJ_INGEST_SIGNAL_METRICS (1u << 1)
#define LJ_INGEST_SIGNAL_TRACES  (1u << 2)
#define LJ_INGEST_SIGNAL_EVENTS  (1u << 3)

// Generic ingest record type enum (lj_ingest_record_v1.record_type).
#define LJ_INGEST_RECORD_TYPE_LOGS    1u
#define LJ_INGEST_RECORD_TYPE_METRICS 2u
#define LJ_INGEST_RECORD_TYPE_TRACES  3u
#define LJ_INGEST_RECORD_TYPE_EVENTS  4u

typedef struct lj_logger lj_logger;

typedef struct lj_ingest_plugin lj_ingest_plugin;
typedef void (*lj_record_callback)(void *user, const struct lj_log_record *record);
typedef void (*lj_generic_record_callback)(void *user, const struct lj_ingest_record_v1 *record);

#ifdef __cplusplus
struct lj_attribute {
    const char *key;
    const char *value;
    int32_t value_type;

    constexpr lj_attribute(const char *k = nullptr, const char *v = nullptr, int32_t t = LJ_ATTR_STRING) : key(k), value(v), value_type(t) {}
};

struct lj_log_record {
    uint64_t timestamp_unix_ns;
    int32_t severity_number;
    const char *severity_text;
    const char *body;
    const lj_attribute *attributes;
    size_t attributes_len;
    const char *event_name;
    const char *service_name;
    const char *scope_name;
    const lj_attribute *resource_attrs;
    size_t resource_attrs_len;
    const lj_attribute *scope_attrs;
    size_t scope_attrs_len;

    constexpr lj_log_record(
        uint64_t ts = 0,
        int32_t sev_no = LJ_SEVERITY_INFO,
        const char *sev_text = nullptr,
        const char *msg = nullptr,
        const lj_attribute *attrs = nullptr,
        size_t attrs_len = 0,
        const char *event = nullptr,
        const char *service = nullptr,
        const char *scope = nullptr,
        const lj_attribute *res_attrs = nullptr,
        size_t res_attrs_len = 0,
        const lj_attribute *scp_attrs = nullptr,
        size_t scp_attrs_len = 0)
        : timestamp_unix_ns(ts),
          severity_number(sev_no),
          severity_text(sev_text),
          body(msg),
          attributes(attrs),
          attributes_len(attrs_len),
          event_name(event),
          service_name(service),
          scope_name(scope),
          resource_attrs(res_attrs),
          resource_attrs_len(res_attrs_len),
          scope_attrs(scp_attrs),
          scope_attrs_len(scp_attrs_len) {}
};
#else
typedef struct lj_attribute {
    const char *key;
    const char *value;
    int32_t value_type;
} lj_attribute;

typedef struct lj_log_record {
    uint64_t timestamp_unix_ns;
    int32_t severity_number;
    const char *severity_text;
    const char *body;
    const lj_attribute *attributes;
    size_t attributes_len;
    const char *event_name;
    const char *service_name;
    const char *scope_name;
    const lj_attribute *resource_attrs;
    size_t resource_attrs_len;
    const lj_attribute *scope_attrs;
    size_t scope_attrs_len;
} lj_log_record;
#endif

#ifdef __cplusplus
struct lj_ingest_record_v1 {
    uint32_t struct_size;
    uint32_t record_type;
    uint64_t timestamp_unix_ns;
    const uint8_t *payload;
    size_t payload_len;
    uint32_t flags;
    uint64_t reserved[4];

    constexpr lj_ingest_record_v1(
        uint32_t sz = 0,
        uint32_t rt = LJ_INGEST_RECORD_TYPE_LOGS,
        uint64_t ts = 0,
        const uint8_t *p = nullptr,
        size_t pl = 0,
        uint32_t f = 0)
        : struct_size(sz),
          record_type(rt),
          timestamp_unix_ns(ts),
          payload(p),
          payload_len(pl),
          flags(f),
          reserved{0, 0, 0, 0} {}
};
#else
typedef struct lj_ingest_record_v1 {
    uint32_t struct_size;
    uint32_t record_type;
    uint64_t timestamp_unix_ns;
    const uint8_t *payload;
    size_t payload_len;
    uint32_t flags;
    uint64_t reserved[4];
} lj_ingest_record_v1;
#endif

const char *lj_version(void);
const char *lj_error_message(void);
lj_logger *lj_logger_new_http(const char *endpoint, const char *service_name, uint64_t timeout_ms);
lj_logger *lj_logger_new_grpc(const char *endpoint, const char *service_name, uint64_t timeout_ms);
// Send one log record. Opens a fresh connection — simplest, most robust.
bool lj_logger_log(lj_logger *logger, const lj_log_record *record);
// Send one record over a persistent gRPC channel or HTTP keep-alive connection.
// First call establishes the connection (slightly slower); subsequent calls reuse it.
bool lj_logger_log_reuse(lj_logger *logger, const lj_log_record *record);
// Send many records in one export request over a persistent connection.
// Records are grouped by service name, resource attributes, and scope.
// A len of 0 or null records is a successful no-op.
bool lj_logger_log_batch(lj_logger *logger, const lj_log_record *records, size_t len);
// Enqueue one record for send on a background runtime; returns immediately.
// Returns false only on validation errors. Network failures are counted via
// lj_logger_async_errors(); records dropped by backpressure via lj_logger_async_dropped().
bool lj_logger_log_async(lj_logger *logger, const lj_log_record *record);
// Enqueue a batch for background send. Same error semantics as lj_logger_log_async.
bool lj_logger_log_batch_async(lj_logger *logger, const lj_log_record *records, size_t len);
// Configure async backpressure. model is LJ_BACKPRESSURE_UNBOUNDED / _DROP / _BLOCK.
// capacity is the max in-flight sends for bounded models (ignored for unbounded).
// Default: LJ_BACKPRESSURE_DROP, capacity 1024. Call before first async send.
bool lj_logger_set_backpressure(lj_logger *logger, int32_t model, size_t capacity);
// Block until all in-flight async sends complete or timeout_ms elapses.
// Returns true if fully drained. Also called by lj_logger_free.
bool lj_logger_flush(lj_logger *logger, uint64_t timeout_ms);
// Count of async sends that failed on the network.
uint64_t lj_logger_async_errors(lj_logger *logger);
// Count of records dropped by bounded backpressure (LJ_BACKPRESSURE_DROP).
uint64_t lj_logger_async_dropped(lj_logger *logger);
// Number of async sends currently in flight.
uint64_t lj_logger_async_inflight(lj_logger *logger);
// Free the logger. Drains in-flight async sends first. Accepts NULL.
void lj_logger_free(lj_logger *logger);

lj_ingest_plugin *lj_ingest_create(void);
void lj_ingest_set_callback(lj_ingest_plugin *ctx, lj_record_callback cb, void *user);
void lj_ingest_set_generic_callback(lj_ingest_plugin *ctx, lj_generic_record_callback cb, void *user);
int lj_ingest_feed(lj_ingest_plugin *ctx, const uint8_t *data, size_t len);
int lj_ingest_fetch(lj_ingest_plugin *ctx);
const char *lj_ingest_last_error(lj_ingest_plugin *ctx);
void lj_ingest_free(lj_ingest_plugin *ctx);

#ifdef __cplusplus
}
#endif

#endif
