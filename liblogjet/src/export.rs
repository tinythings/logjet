//! Stable C ABI definitions for `ljx` exporter plugins.
//!
//! This module deliberately contains only FFI-safe type definitions,
//! constants, and function-pointer signatures. Host and plugin code can both
//! depend on it without sharing any Rust ABI.

use core::ffi::{c_char, c_int, c_void};

/// Exporter ABI major version supported by the host and plugins in this module.
pub const LJX_EXPORTER_ABI_MAJOR: u16 = 1;
/// Exporter ABI minor version supported by the host and plugins in this module.
pub const LJX_EXPORTER_ABI_MINOR: u16 = 0;

/// Fixed symbol name exported by every v1 exporter plugin.
pub const LJX_EXPORTER_DESCRIPTOR_V1_SYMBOL: &[u8] = b"ljx_exporter_descriptor_v1\0";

/// Export completed successfully.
pub const LJX_EXPORT_STATUS_OK: c_int = 0;
/// Export failed with a generic plugin-side error.
pub const LJX_EXPORT_STATUS_ERROR: c_int = -1;
/// Caller supplied an invalid pointer, size, or argument.
pub const LJX_EXPORT_STATUS_BAD_ARG: c_int = -2;
/// Requested operation or input is unsupported by the plugin.
pub const LJX_EXPORT_STATUS_UNSUPPORTED: c_int = -3;
/// Allocation failed inside the plugin.
pub const LJX_EXPORT_STATUS_NOMEM: c_int = -4;
/// I/O through the host write/flush callbacks failed.
pub const LJX_EXPORT_STATUS_IO: c_int = -5;

/// Plugin can process records incrementally and does not need whole-file buffering.
pub const LJX_EXPORT_CAP_STREAMING: u64 = 1 << 0;
/// Plugin accepts `RecordType::Logs` records.
pub const LJX_EXPORT_CAP_RECORD_LOGS: u64 = 1 << 1;
/// Plugin accepts `RecordType::Metrics` records.
pub const LJX_EXPORT_CAP_RECORD_METRICS: u64 = 1 << 2;
/// Plugin accepts `RecordType::Traces` records.
pub const LJX_EXPORT_CAP_RECORD_TRACES: u64 = 1 << 3;
/// Plugin understands OTLP `ExportLogsServiceRequest` protobuf payloads.
pub const LJX_EXPORT_CAP_PAYLOAD_OTLP_EXPORT_LOGS_REQUEST: u64 = 1 << 8;

/// Raw logjet logs record type.
pub const LJX_RECORD_TYPE_LOGS: u32 = 1;
/// Raw logjet metrics record type.
pub const LJX_RECORD_TYPE_METRICS: u32 = 2;
/// Raw logjet traces record type.
pub const LJX_RECORD_TYPE_TRACES: u32 = 3;

/// Payload kind is opaque plugin-defined bytes.
pub const LJX_PAYLOAD_KIND_OPAQUE: u32 = 0;
/// Payload kind is OTLP `ExportLogsServiceRequest` protobuf bytes.
pub const LJX_PAYLOAD_KIND_OTLP_EXPORT_LOGS_REQUEST: u32 = 1;

/// Borrowed string view passed across the C ABI.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LjxAbiString {
    /// Pointer to UTF-8 bytes. May be NULL only when `len == 0`.
    pub ptr: *const c_char,
    /// Number of bytes at `ptr`. The bytes are not required to be NUL-terminated.
    pub len: usize,
}

impl LjxAbiString {
    /// Returns an empty string view.
    pub const fn empty() -> Self {
        Self { ptr: core::ptr::null(), len: 0 }
    }

    /// Builds a string view from a `'static` Rust string.
    pub const fn from_static(value: &'static str) -> Self {
        Self { ptr: value.as_ptr() as *const c_char, len: value.len() }
    }
}

/// Borrowed byte slice view passed across the C ABI.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LjxAbiBytes {
    /// Pointer to the first byte. May be NULL only when `len == 0`.
    pub ptr: *const u8,
    /// Number of bytes at `ptr`.
    pub len: usize,
}

impl LjxAbiBytes {
    /// Returns an empty byte view.
    pub const fn empty() -> Self {
        Self { ptr: core::ptr::null(), len: 0 }
    }

    /// Builds a byte view from a `'static` byte slice.
    pub const fn from_static(value: &'static [u8]) -> Self {
        Self { ptr: value.as_ptr(), len: value.len() }
    }
}

/// Key/value option passed to the exporter at creation time.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LjxExportOptionV1 {
    /// Stable UTF-8 option key.
    pub key: LjxAbiString,
    /// Stable UTF-8 option value.
    pub value: LjxAbiString,
}

/// Host-provided exporter configuration block.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LjxExportInitV1 {
    /// Size of this struct in bytes.
    pub struct_size: u32,
    /// Reserved for future flags. Must be zero in ABI v1.
    pub flags: u32,
    /// Pointer to `options_len` option entries.
    pub options: *const LjxExportOptionV1,
    /// Number of entries in `options`.
    pub options_len: usize,
    /// Reserved extension slots. Must be zeroed by the caller.
    pub reserved: [usize; 4],
}

impl Default for LjxExportInitV1 {
    fn default() -> Self {
        Self { struct_size: core::mem::size_of::<Self>() as u32, flags: 0, options: core::ptr::null(), options_len: 0, reserved: [0; 4] }
    }
}

/// Record view pushed from the host into the exporter.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LjxExportRecordV1 {
    /// Size of this struct in bytes.
    pub struct_size: u32,
    /// Logjet record type: logs, metrics, or traces.
    pub record_type: u32,
    /// Payload encoding kind.
    pub payload_kind: u32,
    /// Reserved for future per-record flags. Must be zero in ABI v1.
    pub flags: u32,
    /// Logjet sequence number.
    pub seq: u64,
    /// Logjet timestamp in Unix nanoseconds.
    pub timestamp_unix_ns: u64,
    /// Borrowed payload bytes valid only for the duration of the call.
    pub payload: LjxAbiBytes,
}

impl Default for LjxExportRecordV1 {
    fn default() -> Self {
        Self {
            struct_size: core::mem::size_of::<Self>() as u32,
            record_type: LJX_RECORD_TYPE_LOGS,
            payload_kind: LJX_PAYLOAD_KIND_OPAQUE,
            flags: 0,
            seq: 0,
            timestamp_unix_ns: 0,
            payload: LjxAbiBytes::empty(),
        }
    }
}

/// Opaque exporter context owned by the plugin implementation.
#[repr(C)]
pub struct LjxExporterCtx {
    _private: [u8; 0],
}

/// Host callback used by the plugin to write output bytes.
pub type LjxExportWriteFn = unsafe extern "C" fn(user: *mut c_void, data: *const u8, len: usize) -> c_int;
/// Host callback used by the plugin to flush buffered output bytes.
pub type LjxExportFlushFn = unsafe extern "C" fn(user: *mut c_void) -> c_int;

/// Host callbacks handed to the plugin at creation time.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LjxExportHostV1 {
    /// Size of this struct in bytes.
    pub struct_size: u32,
    /// Reserved for future flags. Must be zero in ABI v1.
    pub flags: u32,
    /// Opaque pointer forwarded to `write` and `flush`.
    pub user: *mut c_void,
    /// Required write callback.
    pub write: LjxExportWriteFn,
    /// Optional flush callback.
    pub flush: Option<LjxExportFlushFn>,
    /// Reserved extension slots. Must be zeroed by the caller.
    pub reserved: [usize; 6],
}

impl Default for LjxExportHostV1 {
    fn default() -> Self {
        unsafe extern "C" fn default_write(_user: *mut c_void, _data: *const u8, _len: usize) -> c_int {
            LJX_EXPORT_STATUS_BAD_ARG
        }

        Self {
            struct_size: core::mem::size_of::<Self>() as u32,
            flags: 0,
            user: core::ptr::null_mut(),
            write: default_write,
            flush: None,
            reserved: [0; 6],
        }
    }
}

/// Function type: create a new exporter instance bound to the given host callbacks.
pub type LjxExporterCreateFn = unsafe extern "C" fn(host: *const LjxExportHostV1, init: *const LjxExportInitV1) -> *mut LjxExporterCtx;
/// Function type: push one record into the exporter.
pub type LjxExporterWriteRecordFn = unsafe extern "C" fn(ctx: *mut LjxExporterCtx, record: *const LjxExportRecordV1) -> c_int;
/// Function type: finish the export stream.
pub type LjxExporterFinishFn = unsafe extern "C" fn(ctx: *mut LjxExporterCtx) -> c_int;
/// Function type: return the last plugin error as a borrowed UTF-8 string view.
pub type LjxExporterLastErrorFn = unsafe extern "C" fn(ctx: *mut LjxExporterCtx) -> LjxAbiString;
/// Function type: destroy an exporter instance. Accepts NULL.
pub type LjxExporterFreeFn = unsafe extern "C" fn(ctx: *mut LjxExporterCtx);

/// Versioned plugin descriptor discovered by the host through one exported symbol.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LjxExporterDescriptorV1 {
    /// Size of this struct in bytes.
    pub struct_size: u32,
    /// ABI major version implemented by the plugin.
    pub abi_major: u16,
    /// ABI minor version implemented by the plugin.
    pub abi_minor: u16,
    /// Plugin-local implementation version.
    pub plugin_api_version: u32,
    /// Capability bitset advertising accepted record/payload combinations.
    pub capabilities: u64,
    /// Stable machine-readable format name such as `parquet`.
    pub format_name: LjxAbiString,
    /// Human-readable format label such as `Parquet`.
    pub display_name: LjxAbiString,
    /// Default output extension without the leading dot.
    pub default_extension: LjxAbiString,
    /// Create a new exporter context.
    pub create: LjxExporterCreateFn,
    /// Push one record into the exporter.
    pub write_record: LjxExporterWriteRecordFn,
    /// Finish the export stream.
    pub finish: LjxExporterFinishFn,
    /// Read the last error from the exporter.
    pub last_error: LjxExporterLastErrorFn,
    /// Destroy the exporter context.
    pub free: LjxExporterFreeFn,
    /// Reserved extension slots. Must be zeroed by the plugin.
    pub reserved: [usize; 6],
}

impl LjxExporterDescriptorV1 {
    /// Returns the default descriptor header for ABI v1 plugins.
    pub const fn header() -> Self {
        unsafe extern "C" fn create(_host: *const LjxExportHostV1, _init: *const LjxExportInitV1) -> *mut LjxExporterCtx {
            core::ptr::null_mut()
        }
        unsafe extern "C" fn write_record(_ctx: *mut LjxExporterCtx, _record: *const LjxExportRecordV1) -> c_int {
            LJX_EXPORT_STATUS_UNSUPPORTED
        }
        unsafe extern "C" fn finish(_ctx: *mut LjxExporterCtx) -> c_int {
            LJX_EXPORT_STATUS_UNSUPPORTED
        }
        unsafe extern "C" fn last_error(_ctx: *mut LjxExporterCtx) -> LjxAbiString {
            LjxAbiString::empty()
        }
        unsafe extern "C" fn free(_ctx: *mut LjxExporterCtx) {}

        Self {
            struct_size: core::mem::size_of::<Self>() as u32,
            abi_major: LJX_EXPORTER_ABI_MAJOR,
            abi_minor: LJX_EXPORTER_ABI_MINOR,
            plugin_api_version: 1,
            capabilities: 0,
            format_name: LjxAbiString::empty(),
            display_name: LjxAbiString::empty(),
            default_extension: LjxAbiString::empty(),
            create,
            write_record,
            finish,
            last_error,
            free,
            reserved: [0; 6],
        }
    }
}
