#ifndef LIBLOGJET_EXPORT_H
#define LIBLOGJET_EXPORT_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Stable C ABI for ljx exporter plugins.
 *
 * A plugin is a shared library (.so / .dylib) that exports one fixed symbol:
 *   ljx_exporter_descriptor_v1
 *
 * The host loads that symbol, checks the ABI version, inspects the format
 * metadata, then creates an exporter context and pushes raw logjet records to
 * it one by one.
 *
 * Compatibility policy:
 *   - major version changes are breaking
 *   - minor version changes are tail-additive only
 *   - all v1 structs carry struct_size so the host/plugin can safely ignore
 *     fields added in later minor revisions
 *   - all reserved fields must be zeroed by the caller/implementation
 *
 * Ownership rules:
 *   - descriptor strings must point to static storage owned by the plugin
 *   - host-provided init/record buffers are borrowed and valid only for the
 *     duration of the call in which they are passed
 *   - plugins must not free or retain host-owned pointers after a call returns
 *   - last_error returns a borrowed string view valid until the next call on
 *     the same exporter context
 */

enum {
    LJX_EXPORTER_ABI_MAJOR = 1,
    LJX_EXPORTER_ABI_MINOR = 0
};

enum {
    LJX_EXPORT_STATUS_OK          = 0,
    LJX_EXPORT_STATUS_ERROR       = -1,
    LJX_EXPORT_STATUS_BAD_ARG     = -2,
    LJX_EXPORT_STATUS_UNSUPPORTED = -3,
    LJX_EXPORT_STATUS_NOMEM       = -4,
    LJX_EXPORT_STATUS_IO          = -5
};

enum {
    LJX_EXPORT_CAP_STREAMING                        = 1ull << 0,
    LJX_EXPORT_CAP_RECORD_LOGS                      = 1ull << 1,
    LJX_EXPORT_CAP_RECORD_METRICS                   = 1ull << 2,
    LJX_EXPORT_CAP_RECORD_TRACES                    = 1ull << 3,
    LJX_EXPORT_CAP_PAYLOAD_OTLP_EXPORT_LOGS_REQUEST = 1ull << 8
};

enum {
    LJX_RECORD_TYPE_LOGS    = 1,
    LJX_RECORD_TYPE_METRICS = 2,
    LJX_RECORD_TYPE_TRACES  = 3
};

enum {
    LJX_PAYLOAD_KIND_OPAQUE                   = 0,
    LJX_PAYLOAD_KIND_OTLP_EXPORT_LOGS_REQUEST = 1
};

typedef struct ljx_abi_string {
    const char *ptr;
    size_t len;
} ljx_abi_string;

typedef struct ljx_abi_bytes {
    const uint8_t *ptr;
    size_t len;
} ljx_abi_bytes;

typedef struct ljx_export_option_v1 {
    ljx_abi_string key;
    ljx_abi_string value;
} ljx_export_option_v1;

typedef struct ljx_export_init_v1 {
    uint32_t struct_size;
    uint32_t flags;
    const struct ljx_export_option_v1 *options;
    size_t options_len;
    size_t reserved[4];
} ljx_export_init_v1;

typedef struct ljx_export_record_v1 {
    uint32_t struct_size;
    uint32_t record_type;
    uint32_t payload_kind;
    uint32_t flags;
    uint64_t seq;
    uint64_t timestamp_unix_ns;
    ljx_abi_bytes payload;
} ljx_export_record_v1;

typedef struct ljx_exporter_ctx ljx_exporter_ctx;

typedef int32_t (*ljx_export_write_fn)(void *user, const uint8_t *data, size_t len);
typedef int32_t (*ljx_export_flush_fn)(void *user);

typedef struct ljx_export_host_v1 {
    uint32_t struct_size;
    uint32_t flags;
    void *user;
    ljx_export_write_fn write;
    ljx_export_flush_fn flush; /* optional, may be NULL */
    size_t reserved[6];
} ljx_export_host_v1;

typedef ljx_exporter_ctx *(*ljx_exporter_create_fn)(const struct ljx_export_host_v1 *host,
                                                    const struct ljx_export_init_v1 *init);
typedef int32_t (*ljx_exporter_write_record_fn)(ljx_exporter_ctx *ctx,
                                                const struct ljx_export_record_v1 *record);
typedef int32_t (*ljx_exporter_finish_fn)(ljx_exporter_ctx *ctx);
typedef ljx_abi_string (*ljx_exporter_last_error_fn)(ljx_exporter_ctx *ctx);
typedef void (*ljx_exporter_free_fn)(ljx_exporter_ctx *ctx);

typedef struct ljx_exporter_descriptor_v1 {
    uint32_t struct_size;
    uint16_t abi_major;
    uint16_t abi_minor;
    uint32_t plugin_api_version;
    uint64_t capabilities;
    ljx_abi_string format_name;
    ljx_abi_string display_name;
    ljx_abi_string default_extension;
    ljx_exporter_create_fn create;
    ljx_exporter_write_record_fn write_record;
    ljx_exporter_finish_fn finish;
    ljx_exporter_last_error_fn last_error;
    ljx_exporter_free_fn free;
    size_t reserved[6];
} ljx_exporter_descriptor_v1;

/* Fixed exported symbol every v1 exporter plugin must define. */
const struct ljx_exporter_descriptor_v1 *ljx_exporter_descriptor_v1(void);

#ifdef __cplusplus
}
#endif

#endif
