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

// C ABI types (must match liblogjet.h exactly)

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

// Signal constants

const LJ_INGEST_SIGNAL_LOGS: u32 = 1 << 0;
const LJ_INGEST_SIGNAL_METRICS: u32 = 1 << 1;
const LJ_INGEST_SIGNAL_TRACES: u32 = 1 << 2;
const LJ_INGEST_SIGNAL_EVENTS: u32 = 1 << 3;

#[allow(dead_code)]
const LJ_INGEST_RECORD_TYPE_LOGS: u32 = 1;
#[allow(dead_code)]
const LJ_INGEST_RECORD_TYPE_METRICS: u32 = 2;
#[allow(dead_code)]
const LJ_INGEST_RECORD_TYPE_TRACES: u32 = 3;
#[allow(dead_code)]
const LJ_INGEST_RECORD_TYPE_EVENTS: u32 = 4;

// Descriptor

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

// Plugin context

pub struct PerfettoPlugin {
    pub(crate) legacy_callback: Option<RecordCallback>,
    pub(crate) legacy_user: *mut c_void,
    pub(crate) generic_callback: Option<GenericRecordCallback>,
    pub(crate) generic_user: *mut c_void,
    last_error: Option<String>,
}

// Exported C ABI

#[unsafe(no_mangle)]
pub extern "C" fn lj_ingest_create() -> *mut PerfettoPlugin {
    Box::into_raw(Box::new(PerfettoPlugin {
        legacy_callback: None,
        legacy_user: std::ptr::null_mut(),
        generic_callback: None,
        generic_user: std::ptr::null_mut(),
        last_error: None,
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

/// Returns the last error message, or NULL if none.
///
/// # Safety
///
/// `ctx` must be a valid pointer from `lj_ingest_create`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lj_ingest_last_error(ctx: *mut PerfettoPlugin) -> *const c_char {
    if ctx.is_null() {
        return std::ptr::null();
    }
    let ctx = unsafe { &*ctx };
    match &ctx.last_error {
        Some(msg) => msg.as_ptr().cast::<c_char>(),
        None => std::ptr::null(),
    }
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
        eprintln!("perfetto-ingest: lj_ingest_fetch called with null context");
        return -1;
    }
    let ctx = unsafe { &mut *ctx };

    let trace_file = match std::env::var("LJD_PERFETTO_TRACE_FILE") {
        Ok(path) => std::path::PathBuf::from(path),
        Err(_) => {
            ctx.last_error = Some("LJD_PERFETTO_TRACE_FILE is not set".to_string());
            return -2;
        }
    };

    if !trace_file.is_file() {
        ctx.last_error = Some(format!("trace file not found: {}", trace_file.display()));
        return -3;
    }

    match run_pipeline(ctx, &trace_file) {
        Ok(()) => 0,
        Err(err) => {
            ctx.last_error = Some(err.to_string());
            eprintln!("perfetto-ingest: {err}");
            -4
        }
    }
}

fn run_pipeline(plugin: &mut PerfettoPlugin, trace_file: &std::path::Path) -> Result<(), String> {
    let tp_path = perfetto_invoke::find_trace_processor()
        .map_err(|err| format!("trace_processor not found: {err}"))?;

    eprintln!("perfetto-ingest: using trace_processor {}", tp_path.display());
    eprintln!("perfetto-ingest: exporting SQLite from {}", trace_file.display());

    let sqlite_path = perfetto_invoke::export_sqlite(trace_file, &tp_path)
        .map_err(|err| format!("SQLite export failed: {err}"))?;

    let db = sqlite_reader::PerfettoDb::open(&sqlite_path)
        .map_err(|err| format!("failed to open exported DB: {err}"))?;

    let snaps = db.read_clock_snapshots()
        .map_err(|err| format!("failed to read clock snapshots: {err}"))?;

    let policy = match std::env::var("LJD_PERFETTO_TIMESTAMP_POLICY").as_deref() {
        Ok("require-realtime") => timestamp::TimestampPolicy::RequireRealtime,
        _ => timestamp::TimestampPolicy::BestEffort,
    };

    let converter = timestamp::TimestampConverter::new(snaps, policy);

    if converter.has_realtime() {
        eprintln!("perfetto-ingest: realtime clock available");
    } else {
        eprintln!("perfetto-ingest: no realtime clock snapshots — timestamps will be unavailable");
    }

    eprintln!("perfetto-ingest: mapping traces...");
    trace_mapper::map_traces(&db, &converter, emit_generic, plugin)?;

    // Optional: run metrics export and map metrics.
    let metrics_names: Vec<String> = std::env::var("LJD_PERFETTO_METRICS")
        .ok()
        .map(|s| s.split(',').map(|s| s.trim().to_string()).collect::<Vec<_>>())
        .unwrap_or_default();
    let metrics_refs: Vec<&str> = metrics_names.iter().map(|s| s.as_str()).collect();

    if !metrics_refs.is_empty()
        && let Ok(Some(metrics_path)) = perfetto_invoke::export_metrics(trace_file, &tp_path, &metrics_refs) {
            eprintln!("perfetto-ingest: mapping metrics...");
            let metrics = metrics_reader::parse_metrics_json(&metrics_path)
                .map_err(|err| format!("failed to parse metrics JSON: {err}"))?;
            metric_mapper::map_metrics(&metrics, &converter, emit_generic, plugin)?;
            let _ = std::fs::remove_file(&metrics_path);
        }

    eprintln!("perfetto-ingest: mapping logs...");
    log_mapper::map_logs(&db, &converter, emit_generic, plugin)?;

    let _ = std::fs::remove_file(&sqlite_path);

    eprintln!("perfetto-ingest: done");
    Ok(())
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

// Record emission helpers

/// Calls the generic callback with a pre-encoded OTLP payload.
///
/// # Safety
///
/// `ctx` must have a generic callback set.
#[allow(dead_code)]
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
