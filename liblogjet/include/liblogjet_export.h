#ifndef LIBLOGJET_EXPORT_H
#define LIBLOGJET_EXPORT_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define LJX_EXPORTER_ABI_MAJOR 1u
#define LJX_EXPORTER_ABI_MINOR 0u

#define LJX_EXPORT_STATUS_OK 0
#define LJX_EXPORT_STATUS_ERROR 1
#define LJX_EXPORT_STATUS_IO 2
#define LJX_EXPORT_STATUS_BAD_ARG 3
#define LJX_EXPORT_STATUS_UNSUPPORTED 4

#define LJX_EXPORT_CAP_STREAMING (1ull << 0)
#define LJX_EXPORT_CAP_RECORD_LOGS (1ull << 1)
#define LJX_EXPORT_CAP_RECORD_METRICS (1ull << 2)
#define LJX_EXPORT_CAP_RECORD_TRACES (1ull << 3)
#define LJX_EXPORT_CAP_PAYLOAD_OTLP_EXPORT_LOGS_REQUEST (1ull << 4)

#define LJX_RECORD_TYPE_LOGS 1u
#define LJX_RECORD_TYPE_METRICS 2u
#define LJX_RECORD_TYPE_TRACES 3u

#define LJX_PAYLOAD_KIND_OPAQUE 0u
#define LJX_PAYLOAD_KIND_OTLP_EXPORT_LOGS_REQUEST 1u

#define LJX_EXPORTER_DESCRIPTOR_V1_SYMBOL "ljx_exporter_descriptor_v1"

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

typedef struct ljx_exporter_ctx ljx_exporter_ctx;

typedef struct ljx_export_host_v1 {
    uint32_t struct_size;
    uint32_t flags;
    void *user;
    int32_t (*write)(void *user, const uint8_t *data, size_t len);
    int32_t (*flush)(void *user);
    uint64_t reserved[6];
} ljx_export_host_v1;

typedef struct ljx_export_init_v1 {
    uint32_t struct_size;
    uint32_t flags;
    const ljx_export_option_v1 *options;
    size_t options_len;
    uint64_t reserved[4];
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

typedef struct ljx_exporter_descriptor_v1 {
    uint32_t struct_size;
    uint32_t abi_major;
    uint32_t abi_minor;
    uint32_t plugin_api_version;
    uint64_t capabilities;
    ljx_abi_string format_name;
    ljx_abi_string display_name;
    ljx_abi_string default_extension;
    ljx_exporter_ctx *(*create)(const ljx_export_host_v1 *host, const ljx_export_init_v1 *init);
    int32_t (*write_record)(ljx_exporter_ctx *ctx, const ljx_export_record_v1 *record);
    int32_t (*finish)(ljx_exporter_ctx *ctx);
    ljx_abi_string (*last_error)(ljx_exporter_ctx *ctx);
    void (*free)(ljx_exporter_ctx *ctx);
    uint64_t reserved[6];
} ljx_exporter_descriptor_v1;

const ljx_exporter_descriptor_v1 *ljx_exporter_descriptor_v1(void);

#ifdef __cplusplus
}
#endif

#endif
