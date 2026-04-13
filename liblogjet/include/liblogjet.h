#ifndef LIBLOGJET_H
#define LIBLOGJET_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Public C ABI for sending OTLP log records through liblogjet. */

/* Opaque logger handle created by lj_logger_new_http/grpc and freed by lj_logger_free. */
typedef struct lj_logger lj_logger;

/* Selected OTLP severity_number values for common log levels. */
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
    /* OTel severity_text, for example "INFO"; UTF-8, NUL-terminated, may be NULL */
    const char *severity_text;
    /* OTel body string; UTF-8, NUL-terminated, must not be NULL */
    const char *body;
    /* Arbitrary OTLP LogRecord string attributes; may be NULL only when attributes_len == 0 */
    const struct lj_attribute *attributes;
    /* Number of entries in attributes */
    size_t attributes_len;
} lj_log_record;

/* Returns the liblogjet version string as a static NUL-terminated string. */
const char *lj_version(void);

/* Returns the calling thread's last liblogjet error message as a static string view. */
const char *lj_error_message(void);

/* Creates an OTLP/HTTP logger.
 * endpoint must be UTF-8 and NUL-terminated, using http://host:port[/path] or bare host:port[/path].
 * https:// is rejected for this constructor. Returns NULL on failure.
 */
lj_logger *lj_logger_new_http(const char *endpoint, const char *service_name, uint64_t timeout_ms);

/* Creates an OTLP/gRPC logger.
 * endpoint must be UTF-8 and NUL-terminated, using host:port or an explicit http/https URL.
 * Returns NULL on failure.
 */
lj_logger *lj_logger_new_grpc(const char *endpoint, const char *service_name, uint64_t timeout_ms);

/* Frees a logger created by lj_logger_new_http or lj_logger_new_grpc. Accepts NULL. */
void lj_logger_free(lj_logger *logger);

/* Sends one OTLP log record through logger.
 * logger and record must be valid for the duration of the call.
 * Returns false on failure; inspect lj_error_message() for details.
 */
bool lj_logger_log(lj_logger *logger, const lj_log_record *record);

/* ---------------------------------------------------------------------------
 * Inbound ingest plugin ABI.
 *
 * A plugin is a shared library (.so / .dylib) that ljd dlopen's at startup.
 * ljd feeds raw TCP bytes into the plugin; the plugin parses them and calls
 * back with lj_log_record structs (same struct as outbound, both directions).
 *
 * The plugin must export these four symbols:
 *   lj_ingest_create        — allocate parsing context
 *   lj_ingest_set_callback  — register the record-delivery callback
 *   lj_ingest_feed          — push raw bytes into the parser
 *   lj_ingest_free          — destroy the parsing context
 * --------------------------------------------------------------------------- */

/* Opaque plugin context created by lj_ingest_create and freed by lj_ingest_free. */
typedef struct lj_ingest_plugin lj_ingest_plugin;

/* Callback invoked by the plugin for each parsed record.
 * `user` is the opaque pointer passed to lj_ingest_set_callback.
 * The record pointer is only valid for the duration of the callback.
 */
typedef void (*lj_record_callback)(void *user, const lj_log_record *record);

/* Creates a new plugin parsing context. Returns NULL on failure. */
lj_ingest_plugin *lj_ingest_create(void);

/* Registers the callback that the plugin calls for each parsed record.
 * `user` is forwarded as-is to every callback invocation.
 */
void lj_ingest_set_callback(lj_ingest_plugin *ctx,
                            lj_record_callback cb, void *user);

/* Feeds raw bytes from a TCP stream into the plugin parser.
 * Returns 0 on success, non-zero on unrecoverable parse error.
 */
int lj_ingest_feed(lj_ingest_plugin *ctx,
                   const uint8_t *data, size_t len);

/* Optional. Active-source plugins export this instead of expecting
 * lj_ingest_feed calls. ljd calls lj_ingest_fetch once; the plugin
 * takes over, reads from its own source (device buffer, file, etc.),
 * and delivers records through the callback. Blocks until the source
 * is exhausted or an error occurs.
 * Returns 0 on success, non-zero on error.
 * If a plugin does NOT export this symbol, ljd uses passive TCP mode
 * with lj_ingest_feed.
 */
int lj_ingest_fetch(lj_ingest_plugin *ctx);

/* Destroys the plugin context. Accepts NULL. */
void lj_ingest_free(lj_ingest_plugin *ctx);

#ifdef __cplusplus
}
#endif

#endif
