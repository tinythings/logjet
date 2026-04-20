//! Stable Rust-side ABI shared between `ljx` and exporter plugins.

use std::ffi::{c_char, c_void};

/// Current exporter ABI major version.
pub const LJX_EXPORTER_ABI_MAJOR: u32 = 1;
/// Current exporter ABI minor version.
pub const LJX_EXPORTER_ABI_MINOR: u32 = 0;

/// Symbol name every exporter plugin must expose.
pub const LJX_EXPORTER_DESCRIPTOR_V1_SYMBOL: &[u8] = b"ljx_exporter_descriptor_v1\0";

/// Export completed successfully.
pub const LJX_EXPORT_STATUS_OK: i32 = 0;
/// Export failed with a generic plugin error.
pub const LJX_EXPORT_STATUS_ERROR: i32 = 1;
/// Export failed with an I/O error.
pub const LJX_EXPORT_STATUS_IO: i32 = 2;
/// Export failed because the host passed a bad argument.
pub const LJX_EXPORT_STATUS_BAD_ARG: i32 = 3;
/// Export failed because the plugin does not support the request.
pub const LJX_EXPORT_STATUS_UNSUPPORTED: i32 = 4;

/// Plugin can stream output incrementally.
pub const LJX_EXPORT_CAP_STREAMING: u64 = 1 << 0;
/// Plugin accepts log records.
pub const LJX_EXPORT_CAP_RECORD_LOGS: u64 = 1 << 1;
/// Plugin accepts metric records.
pub const LJX_EXPORT_CAP_RECORD_METRICS: u64 = 1 << 2;
/// Plugin accepts trace records.
pub const LJX_EXPORT_CAP_RECORD_TRACES: u64 = 1 << 3;
/// Plugin understands OTLP `ExportLogsServiceRequest` payloads.
pub const LJX_EXPORT_CAP_PAYLOAD_OTLP_EXPORT_LOGS_REQUEST: u64 = 1 << 4;

/// Record contains logs.
pub const LJX_RECORD_TYPE_LOGS: u32 = 1;
/// Record contains metrics.
pub const LJX_RECORD_TYPE_METRICS: u32 = 2;
/// Record contains traces.
pub const LJX_RECORD_TYPE_TRACES: u32 = 3;

/// Payload is opaque bytes.
pub const LJX_PAYLOAD_KIND_OPAQUE: u32 = 0;
/// Payload is an OTLP `ExportLogsServiceRequest` protobuf.
pub const LJX_PAYLOAD_KIND_OTLP_EXPORT_LOGS_REQUEST: u32 = 1;

/// Pointer/length string view used by the exporter ABI.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LjxAbiString {
    /// UTF-8 bytes pointer. May be null when `len == 0`.
    pub ptr: *const c_char,
    /// Byte length excluding the trailing NUL.
    pub len: usize,
}

impl LjxAbiString {
    /// Builds an ABI string from a static Rust string.
    pub const fn from_static(value: &'static str) -> Self {
        Self { ptr: value.as_ptr().cast::<c_char>(), len: value.len() }
    }
}

/// Pointer/length byte view used by the exporter ABI.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LjxAbiBytes {
    /// Byte pointer. May be null when `len == 0`.
    pub ptr: *const u8,
    /// Byte length.
    pub len: usize,
}

impl LjxAbiBytes {
    /// Builds an ABI bytes view from a Rust slice.
    pub const fn from_slice(value: &'static [u8]) -> Self {
        Self { ptr: value.as_ptr(), len: value.len() }
    }
}

/// One key/value initialisation option passed to an exporter plugin.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LjxExportOptionV1 {
    /// Option name.
    pub key: LjxAbiString,
    /// Option value.
    pub value: LjxAbiString,
}

/// Host callback table passed to an exporter plugin.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LjxExportHostV1 {
    /// Structure size for forward-compat checks.
    pub struct_size: u32,
    /// Reserved host flags.
    pub flags: u32,
    /// Opaque host pointer passed back to callbacks.
    pub user: *mut c_void,
    /// Writes bytes into the host-owned output stream.
    pub write: unsafe extern "C" fn(user: *mut c_void, data: *const u8, len: usize) -> i32,
    /// Optional flush callback.
    pub flush: Option<unsafe extern "C" fn(user: *mut c_void) -> i32>,
    /// Reserved for future ABI growth.
    pub reserved: [u64; 6],
}

impl Default for LjxExportHostV1 {
    fn default() -> Self {
        Self {
            struct_size: std::mem::size_of::<Self>() as u32,
            flags: 0,
            user: std::ptr::null_mut(),
            write: noop_write,
            flush: Some(noop_flush),
            reserved: [0; 6],
        }
    }
}

/// Export initialisation data passed once at plugin creation time.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LjxExportInitV1 {
    /// Structure size for forward-compat checks.
    pub struct_size: u32,
    /// Reserved init flags.
    pub flags: u32,
    /// Pointer to `options_len` options.
    pub options: *const LjxExportOptionV1,
    /// Number of options.
    pub options_len: usize,
    /// Reserved for future ABI growth.
    pub reserved: [u64; 4],
}

impl Default for LjxExportInitV1 {
    fn default() -> Self {
        Self { struct_size: std::mem::size_of::<Self>() as u32, flags: 0, options: std::ptr::null(), options_len: 0, reserved: [0; 4] }
    }
}

/// One record handed from the host to the exporter plugin.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct LjxExportRecordV1 {
    /// Structure size for forward-compat checks.
    pub struct_size: u32,
    /// One of `LJX_RECORD_TYPE_*`.
    pub record_type: u32,
    /// One of `LJX_PAYLOAD_KIND_*`.
    pub payload_kind: u32,
    /// Reserved per-record flags.
    pub flags: u32,
    /// Sequence number from the source file.
    pub seq: u64,
    /// Timestamp in Unix nanoseconds.
    pub timestamp_unix_ns: u64,
    /// Encoded payload bytes.
    pub payload: LjxAbiBytes,
}

/// Opaque exporter context owned by the plugin.
#[repr(C)]
pub struct LjxExporterCtx {
    _private: [u8; 0],
}

/// Exporter descriptor returned by `ljx_exporter_descriptor_v1`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LjxExporterDescriptorV1 {
    /// Structure size for forward-compat checks.
    pub struct_size: u32,
    /// ABI major version.
    pub abi_major: u32,
    /// ABI minor version.
    pub abi_minor: u32,
    /// Plugin-specific API version.
    pub plugin_api_version: u32,
    /// Capability bitset.
    pub capabilities: u64,
    /// Stable machine-readable format name.
    pub format_name: LjxAbiString,
    /// Human-readable display name.
    pub display_name: LjxAbiString,
    /// Default file extension without a dot.
    pub default_extension: LjxAbiString,
    /// Creates a plugin exporter context.
    pub create: unsafe extern "C" fn(host: *const LjxExportHostV1, init: *const LjxExportInitV1) -> *mut LjxExporterCtx,
    /// Writes one record.
    pub write_record: unsafe extern "C" fn(ctx: *mut LjxExporterCtx, record: *const LjxExportRecordV1) -> i32,
    /// Finalises the export.
    pub finish: unsafe extern "C" fn(ctx: *mut LjxExporterCtx) -> i32,
    /// Returns the last plugin error message.
    pub last_error: unsafe extern "C" fn(ctx: *mut LjxExporterCtx) -> LjxAbiString,
    /// Frees the exporter context.
    pub free: unsafe extern "C" fn(ctx: *mut LjxExporterCtx),
    /// Reserved for future ABI growth.
    pub reserved: [u64; 6],
}

impl LjxExporterDescriptorV1 {
    /// Builds a zeroed descriptor header with the current ABI defaults.
    pub fn header() -> Self {
        Self {
            struct_size: std::mem::size_of::<Self>() as u32,
            abi_major: LJX_EXPORTER_ABI_MAJOR,
            abi_minor: LJX_EXPORTER_ABI_MINOR,
            plugin_api_version: 1,
            capabilities: 0,
            format_name: LjxAbiString::default(),
            display_name: LjxAbiString::default(),
            default_extension: LjxAbiString::default(),
            create: noop_create,
            write_record: noop_write_record,
            finish: noop_finish,
            last_error: noop_last_error,
            free: noop_free,
            reserved: [0; 6],
        }
    }
}

unsafe extern "C" fn noop_write(_user: *mut c_void, _data: *const u8, _len: usize) -> i32 {
    LJX_EXPORT_STATUS_OK
}

unsafe extern "C" fn noop_flush(_user: *mut c_void) -> i32 {
    LJX_EXPORT_STATUS_OK
}

unsafe extern "C" fn noop_create(_host: *const LjxExportHostV1, _init: *const LjxExportInitV1) -> *mut LjxExporterCtx {
    std::ptr::null_mut()
}

unsafe extern "C" fn noop_write_record(_ctx: *mut LjxExporterCtx, _record: *const LjxExportRecordV1) -> i32 {
    LJX_EXPORT_STATUS_UNSUPPORTED
}

unsafe extern "C" fn noop_finish(_ctx: *mut LjxExporterCtx) -> i32 {
    LJX_EXPORT_STATUS_OK
}

unsafe extern "C" fn noop_last_error(_ctx: *mut LjxExporterCtx) -> LjxAbiString {
    LjxAbiString::default()
}

unsafe extern "C" fn noop_free(_ctx: *mut LjxExporterCtx) {}

#[cfg(test)]
#[path = "../tests/unit/export_ut.rs"]
mod export_ut;
