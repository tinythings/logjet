#ifndef LIBLOGJET_H
#define LIBLOGJET_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct lj_logger lj_logger;

enum {
    LJ_SEVERITY_TRACE = 1,
    LJ_SEVERITY_DEBUG = 5,
    LJ_SEVERITY_INFO = 9,
    LJ_SEVERITY_WARN = 13,
    LJ_SEVERITY_ERROR = 17,
    LJ_SEVERITY_FATAL = 21
};

typedef struct lj_attribute {
    /* OTLP LogRecord attribute key */
    const char *key;
    /* OTLP LogRecord attribute value as UTF-8 string */
    const char *value;
} lj_attribute;

typedef struct lj_log_record {
    /* OTel time_unix_nano */
    uint64_t timestamp_unix_ns;
    /* OTel severity_number, for example LJ_SEVERITY_INFO */
    int32_t severity_number;
    /* OTel severity_text, for example "INFO"; may be NULL */
    const char *severity_text;
    /* OTel body string */
    const char *body;
    /* Arbitrary OTLP LogRecord string attributes */
    const struct lj_attribute *attributes;
    size_t attributes_len;
} lj_log_record;

const char *lj_version(void);
const char *lj_error_message(void);
lj_logger *lj_logger_new_http(const char *endpoint, const char *service_name, uint64_t timeout_ms);
lj_logger *lj_logger_new_grpc(const char *endpoint, const char *service_name, uint64_t timeout_ms);
void lj_logger_free(lj_logger *logger);
bool lj_logger_log(lj_logger *logger, const lj_log_record *record);

#ifdef __cplusplus
}
#endif

#endif
