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

typedef struct lj_logger lj_logger;

typedef struct lj_ingest_plugin lj_ingest_plugin;
typedef void (*lj_record_callback)(void *user, const struct lj_log_record *record);

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

const char *lj_version(void);
const char *lj_error_message(void);
lj_logger *lj_logger_new_http(const char *endpoint, const char *service_name, uint64_t timeout_ms);
lj_logger *lj_logger_new_grpc(const char *endpoint, const char *service_name, uint64_t timeout_ms);
bool lj_logger_log(lj_logger *logger, const lj_log_record *record);
void lj_logger_free(lj_logger *logger);

lj_ingest_plugin *lj_ingest_create(void);
void lj_ingest_set_callback(lj_ingest_plugin *ctx, lj_record_callback cb, void *user);
int lj_ingest_feed(lj_ingest_plugin *ctx, const uint8_t *data, size_t len);
int lj_ingest_fetch(lj_ingest_plugin *ctx);
void lj_ingest_free(lj_ingest_plugin *ctx);

#ifdef __cplusplus
}
#endif

#endif
