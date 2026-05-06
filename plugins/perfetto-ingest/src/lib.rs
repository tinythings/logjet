//! Perfetto trace ingest plugin for ljd.
//!
//! Active-source plugin: exports `lj_ingest_fetch` which reads a `.pftrace`
//! file, invokes Perfetto `trace_processor` for analysis, maps results into
//! OTel traces/metrics/logs/events, and streams pre-encoded OTLP payloads
//! through the generic record callback.

mod log_mapper;
mod metric_mapper;
mod metrics_reader;
mod perfetto_invoke;
mod sqlite_reader;
mod timestamp;
mod trace_mapper;

use std::ffi::{c_char, c_int, c_void};

// ── C ABI types (must match liblogjet.h exactly) ────────────────────────────

#[repr(C)]
pub struct LjAttribute {
    key: *const c_char,
    value: *const c_char,
    value_type: i32,
}

#[repr(C)]
pub struct LjLogRecord {
    timestamp_unix_ns: u64,
    severity_number: i32,
    severity_text: *const c_char,
    body: *const c_char,
    attributes: *const LjAttribute,
    attributes_len: usize,
    event_name: *const c_char,
    service_name: *const c_char,
    scope_name: *const c_char,
    resource_attrs: *const LjAttribute,
    resource_attrs_len: usize,
    scope_attrs: *const LjAttribute,
    scope_attrs_len: usize,
}

#[repr(C)]
pub struct LjIngestRecordV1 {
    struct_size: u32,
    record_type: u32,
    timestamp_unix_ns: u64,
    payload: *const u8,
    payload_len: usize,
    flags: u32,
    reserved: [u64; 4],
}

pub type RecordCallback = unsafe extern "C" fn(*mut c_void, *const LjLogRecord);
pub type GenericRecordCallback = unsafe extern "C" fn(*mut c_void, *const LjIngestRecordV1);

#[repr(C)]
pub struct LjIngestDescriptorV1 {
    struct_size: u32,
    abi_major: u32,
    abi_minor: u32,
    name: *const c_char,
    display_name: *const c_char,
    mode: u32,
    reserved: [u64; 8],
}

// ── Signal constants ────────────────────────────────────────────────────────

const LJ_INGEST_SIGNAL_LOGS: u32 = 1 << 0;
const LJ_INGEST_SIGNAL_METRICS: u32 = 1 << 1;
const LJ_INGEST_SIGNAL_TRACES: u32 = 1 << 2;
const LJ_INGEST_SIGNAL_EVENTS: u32 = 1 << 3;

const LJ_INGEST_RECORD_TYPE_LOGS: u32 = 1;
const LJ_INGEST_RECORD_TYPE_METRICS: u32 = 2;
const LJ_INGEST_RECORD_TYPE_TRACES: u32 = 3;
const LJ_INGEST_RECORD_TYPE_EVENTS: u32 = 4;

// ── Descriptor ──────────────────────────────────────────────────────────────

struct IngestDescriptor(LjIngestDescriptorV1);

unsafe impl Sync for IngestDescriptor {}

static PERFETTO_INGEST_DESCRIPTOR: IngestDescriptor = IngestDescriptor(LjIngestDescriptorV1 {
    struct_size: std::mem::size_of::<LjIngestDescriptorV1>() as u32,
    abi_major: 1,
    abi_minor: 1,
    name: c"perfetto".as_ptr(),
    display_name: c"Perfetto trace importer".as_ptr(),
    mode: 1,
    reserved: {
        let mut r = [0u64; 8];
        r[0] = (LJ_INGEST_SIGNAL_LOGS | LJ_INGEST_SIGNAL_METRICS | LJ_INGEST_SIGNAL_TRACES | LJ_INGEST_SIGNAL_EVENTS) as u64;
        r
    },
});

#[unsafe(no_mangle)]
pub extern "C" fn lj_ingest_descriptor_v1() -> *const LjIngestDescriptorV1 {
    &PERFETTO_INGEST_DESCRIPTOR.0
}

// ── Plugin context ──────────────────────────────────────────────────────────

pub struct PerfettoPlugin {
    legacy_callback: Option<RecordCallback>,
    legacy_user: *mut c_void,
    generic_callback: Option<GenericRecordCallback>,
    generic_user: *mut c_void,
}

// ── Exported C ABI ──────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn lj_ingest_create() -> *mut PerfettoPlugin {
    Box::into_raw(Box::new(PerfettoPlugin {
        legacy_callback: None,
        legacy_user: std::ptr::null_mut(),
        generic_callback: None,
        generic_user: std::ptr::null_mut(),
    }))
}

/// # Safety
///
/// `ctx` must be a valid pointer from `lj_ingest_create` or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lj_ingest_set_callback(ctx: *mut PerfettoPlugin, cb: RecordCallback, user: *mut c_void) {
    if ctx.is_null() {
        return;
    }
    let ctx = unsafe { &mut *ctx };
    ctx.legacy_callback = Some(cb);
    ctx.legacy_user = user;
}

/// # Safety
///
/// `ctx` must be a valid pointer from `lj_ingest_create` or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lj_ingest_set_generic_callback(ctx: *mut PerfettoPlugin, cb: GenericRecordCallback, user: *mut c_void) {
    if ctx.is_null() {
        return;
    }
    let ctx = unsafe { &mut *ctx };
    ctx.generic_callback = Some(cb);
    ctx.generic_user = user;
}

/// Passive feed — not used by this plugin but required by the ABI.
///
/// # Safety
///
/// Pointers must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lj_ingest_feed(_ctx: *mut PerfettoPlugin, _data: *const u8, _len: usize) -> c_int {
    0
}

/// Active source: reads a `.pftrace` file, invokes trace_processor, maps
/// results to OTel, and streams records through the generic callback.
///
/// # Safety
///
/// `ctx` must be a valid pointer from `lj_ingest_create` with a callback set.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lj_ingest_fetch(ctx: *mut PerfettoPlugin) -> c_int {
    if ctx.is_null() {
        return -1;
    }
    // Stub — real implementation in Ticket 13.
    0
}

/// Destroys the plugin context. Accepts NULL.
///
/// # Safety
///
/// `ctx` must be null or a valid pointer that has not been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lj_ingest_free(ctx: *mut PerfettoPlugin) {
    if ctx.is_null() {
        return;
    }
    let _ = unsafe { Box::from_raw(ctx) };
}

// ── Record emission helpers ─────────────────────────────────────────────────

/// Calls the generic callback with a pre-encoded OTLP payload.
///
/// # Safety
///
/// `ctx` must have a generic callback set.
pub(crate) unsafe fn emit_generic(
    ctx: &PerfettoPlugin,
    record_type: u32,
    ts_unix_ns: u64,
    payload: &[u8],
) {
    let Some(cb) = ctx.generic_callback else {
        return;
    };
    let record = LjIngestRecordV1 {
        struct_size: std::mem::size_of::<LjIngestRecordV1>() as u32,
        record_type,
        timestamp_unix_ns: ts_unix_ns,
        payload: payload.as_ptr(),
        payload_len: payload.len(),
        flags: 0,
        reserved: [0; 4],
    };
    unsafe { cb(ctx.generic_user, &record) };
}

#[cfg(test)]
#[path = "../tests/unit/tests.rs"]
mod tests;
