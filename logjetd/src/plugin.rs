//! Ingest plugin loader.
//!
//! Opens a shared library that implements the `lj_ingest_*` C ABI defined in
//! `liblogjet.h`, accepts raw TCP connections, feeds bytes into the plugin
//! parser, and appends parsed records to the spool.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::io::{self, BufReader, Read};
use std::net::TcpListener;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::protocol::WireRecord;

// ── C ABI types mirroring liblogjet.h ───────────────────────────────────────

#[allow(dead_code)]
const LJ_ATTR_STRING: i32 = 0;
const LJ_ATTR_INT: i32 = 1;
const LJ_ATTR_ARRAY: i32 = 2;

#[repr(C)]
struct LjAttribute {
    key: *const c_char,
    value: *const c_char,
    value_type: i32,
}

#[repr(C)]
struct LjLogRecord {
    timestamp_unix_ns: u64,
    severity_number: i32,
    severity_text: *const c_char,
    body: *const c_char,
    attributes: *const LjAttribute,
    attributes_len: usize,
    // Extended OTel fields (NULL = legacy flat mode)
    event_name: *const c_char,
    service_name: *const c_char,
    scope_name: *const c_char,
    resource_attrs: *const LjAttribute,
    resource_attrs_len: usize,
    scope_attrs: *const LjAttribute,
    scope_attrs_len: usize,
}

/// Opaque plugin context. We never dereference it — just pass the pointer.
enum LjIngestPlugin {}

type CreateFn = unsafe extern "C" fn() -> *mut LjIngestPlugin;
type SetCallbackFn = unsafe extern "C" fn(*mut LjIngestPlugin, RecordCallback, *mut c_void);
type FeedFn = unsafe extern "C" fn(*mut LjIngestPlugin, *const u8, usize) -> c_int;
type FetchFn = unsafe extern "C" fn(*mut LjIngestPlugin) -> c_int;
type FreeFn = unsafe extern "C" fn(*mut LjIngestPlugin);
type RecordCallback = unsafe extern "C" fn(*mut c_void, *const LjLogRecord);

// ── Plugin handle ───────────────────────────────────────────────────────────

/// Resolved symbols from a loaded ingest plugin.
struct PluginHandle {
    _lib: libloading::Library,
    create: CreateFn,
    set_callback: SetCallbackFn,
    feed: FeedFn,
    /// Active-source plugins export `lj_ingest_fetch`. If present, ljd calls
    /// it instead of accepting TCP connections and calling `lj_ingest_feed`.
    fetch: Option<FetchFn>,
    free: FreeFn,
}

impl PluginHandle {
    /// Loads the shared library at `path` and resolves required symbols.
    fn load(path: &Path) -> io::Result<Self> {
        // SAFETY: we trust the operator-provided .so path.
        let lib = unsafe { libloading::Library::new(path) }.map_err(|err| io::Error::other(format!("dlopen {}: {err}", path.display())))?;

        // SAFETY: symbol signatures must match the lj_ingest_* ABI.
        unsafe {
            let create: libloading::Symbol<CreateFn> =
                lib.get(b"lj_ingest_create\0").map_err(|err| io::Error::other(format!("symbol lj_ingest_create: {err}")))?;
            let set_callback: libloading::Symbol<SetCallbackFn> =
                lib.get(b"lj_ingest_set_callback\0").map_err(|err| io::Error::other(format!("symbol lj_ingest_set_callback: {err}")))?;
            let feed: libloading::Symbol<FeedFn> =
                lib.get(b"lj_ingest_feed\0").map_err(|err| io::Error::other(format!("symbol lj_ingest_feed: {err}")))?;
            let fetch: Option<FetchFn> = lib.get::<FetchFn>(b"lj_ingest_fetch\0").ok().map(|sym| *sym);
            let free: libloading::Symbol<FreeFn> =
                lib.get(b"lj_ingest_free\0").map_err(|err| io::Error::other(format!("symbol lj_ingest_free: {err}")))?;

            Ok(Self { create: *create, set_callback: *set_callback, feed: *feed, fetch, free: *free, _lib: lib })
        }
    }

    /// Returns true if the plugin is an active source (exports `lj_ingest_fetch`).
    fn is_active(&self) -> bool {
        self.fetch.is_some()
    }
}

// ── Callback plumbing ───────────────────────────────────────────────────────

/// Passed through the `void *user` pointer in the C callback.
struct CallbackCtx {
    spool: Arc<super::daemon::SharedSpool>,
    next_seq: Arc<AtomicU64>,
}

/// The C callback invoked by the plugin for each parsed record.
///
/// # Safety
///
/// `user` must be a valid `*mut CallbackCtx`. `record` must be a valid
/// `*const LjLogRecord` with all nested pointers valid for the call duration.
unsafe extern "C" fn on_record(user: *mut c_void, record: *const LjLogRecord) {
    let ctx = unsafe { &*(user as *const CallbackCtx) };
    let rec = unsafe { &*record };

    let body = if rec.body.is_null() { String::new() } else { unsafe { CStr::from_ptr(rec.body) }.to_string_lossy().into_owned() };

    let severity_text =
        if rec.severity_text.is_null() { None } else { Some(unsafe { CStr::from_ptr(rec.severity_text) }.to_string_lossy().into_owned()) };

    let attrs = unsafe { read_attrs(rec.attributes, rec.attributes_len) };

    let ts = if rec.timestamp_unix_ns != 0 {
        rec.timestamp_unix_ns
    } else {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos() as u64
    };

    // Extended OTel fields (NULL = legacy flat mode).
    let event_name = unsafe { read_optional_str(rec.event_name) };
    let service_name = unsafe { read_optional_str(rec.service_name) };
    let scope_name = unsafe { read_optional_str(rec.scope_name) };
    let resource_attrs = unsafe { read_attrs(rec.resource_attrs, rec.resource_attrs_len) };
    let scope_attrs = unsafe { read_attrs(rec.scope_attrs, rec.scope_attrs_len) };

    let payload = build_otlp_payload(OtlpRecord {
        ts,
        severity: rec.severity_number,
        severity_text: severity_text.as_deref(),
        body: &body,
        attrs: &attrs,
        event_name: event_name.as_deref(),
        service_name: service_name.as_deref(),
        scope_name: scope_name.as_deref(),
        resource_attrs: &resource_attrs,
        scope_attrs: &scope_attrs,
    });

    let wire = WireRecord { record_type: logjet::RecordType::Logs, seq: ctx.next_seq.fetch_add(1, Ordering::Relaxed), ts_unix_ns: ts, payload };

    if let Err(err) = super::daemon::append_to_spool(&ctx.spool, wire) {
        eprintln!("ljd plugin callback spool error: {err}");
    }
}

/// Reads a NUL-terminated C string, returns None if the pointer is null.
unsafe fn read_optional_str(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() { None } else { Some(unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned()) }
}

/// Reads an array of LjAttribute into owned (key, value, value_type) triples.
unsafe fn read_attrs(ptr: *const LjAttribute, len: usize) -> Vec<(String, String, i32)> {
    let mut out = Vec::new();
    if !ptr.is_null() && len > 0 {
        let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
        for attr in slice {
            if attr.key.is_null() || attr.value.is_null() {
                continue;
            }
            let key = unsafe { CStr::from_ptr(attr.key) }.to_string_lossy().into_owned();
            let val = unsafe { CStr::from_ptr(attr.value) }.to_string_lossy().into_owned();
            out.push((key, val, attr.value_type));
        }
    }
    out
}

/// Input for OTLP payload construction.
pub(crate) struct OtlpRecord<'a> {
    pub ts: u64,
    pub severity: i32,
    pub severity_text: Option<&'a str>,
    pub body: &'a str,
    pub attrs: &'a [(String, String, i32)],
    // Extended (all None = legacy flat mode)
    pub event_name: Option<&'a str>,
    pub service_name: Option<&'a str>,
    pub scope_name: Option<&'a str>,
    pub resource_attrs: &'a [(String, String, i32)],
    pub scope_attrs: &'a [(String, String, i32)],
}

/// Encodes a single log record as an OTLP ExportLogsServiceRequest protobuf.
///
/// When `service_name` / `scope_name` / `event_name` are provided, builds a
/// spec-compliant OTLP structure with proper Resource and Scope. Otherwise
/// falls back to a minimal flat-attribute wrapper.
pub(crate) fn build_otlp_payload(rec: OtlpRecord<'_>) -> Vec<u8> {
    use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
    use opentelemetry_proto::tonic::common::v1::any_value::Value;
    use opentelemetry_proto::tonic::common::v1::{AnyValue, ArrayValue, InstrumentationScope, KeyValue};
    use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
    use opentelemetry_proto::tonic::resource::v1::Resource;
    use prost::Message;

    let to_kv = |(k, v, t): &(String, String, i32)| KeyValue {
        key: k.clone(),
        value: Some(match *t {
            LJ_ATTR_INT => AnyValue { value: Some(Value::IntValue(v.parse::<i64>().unwrap_or(0))) },
            LJ_ATTR_ARRAY => AnyValue {
                value: Some(Value::ArrayValue(ArrayValue {
                    values: v.split(',').map(|s| AnyValue { value: Some(Value::StringValue(s.to_string())) }).collect(),
                })),
            },
            _ => AnyValue { value: Some(Value::StringValue(v.clone())) },
        }),
    };

    let record = LogRecord {
        time_unix_nano: rec.ts,
        observed_time_unix_nano: rec.ts,
        severity_number: rec.severity,
        severity_text: rec.severity_text.unwrap_or_default().to_string(),
        body: Some(AnyValue { value: Some(Value::StringValue(rec.body.to_string())) }),
        attributes: rec.attrs.iter().map(to_kv).collect(),
        dropped_attributes_count: 0,
        flags: 0,
        trace_id: Vec::new(),
        span_id: Vec::new(),
        event_name: rec.event_name.unwrap_or_default().to_string(),
    };

    // Extended mode: proper Resource + Scope from plugin-provided fields.
    let has_extended = rec.service_name.is_some() || rec.scope_name.is_some();

    let mut resource_kv: Vec<KeyValue> = rec.resource_attrs.iter().map(to_kv).collect();
    if let Some(svc) = rec.service_name {
        resource_kv.insert(0, KeyValue { key: "service.name".to_string(), value: Some(AnyValue { value: Some(Value::StringValue(svc.to_string())) }) });
    }

    let scope = InstrumentationScope {
        name: if has_extended { rec.scope_name.unwrap_or_default().to_string() } else { "lj-ingest-plugin".to_string() },
        version: String::new(),
        attributes: rec.scope_attrs.iter().map(to_kv).collect(),
        dropped_attributes_count: 0,
    };

    let request = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource { attributes: resource_kv, dropped_attributes_count: 0, entity_refs: Vec::new() }),
            scope_logs: vec![ScopeLogs { scope: Some(scope), log_records: vec![record], schema_url: String::new() }],
            schema_url: String::new(),
        }],
    };

    request.encode_to_vec()
}

// ── Public entry point ──────────────────────────────────────────────────────

/// Runs the plugin ingest loop: loads the .so, then either calls
/// `lj_ingest_fetch` (active plugin) or binds TCP and feeds bytes (passive).
pub fn plugin_ingest_loop(bind_addr: &str, plugin_path: &Path, spool: Arc<super::daemon::SharedSpool>, next_seq: Arc<AtomicU64>) -> io::Result<()> {
    let handle = Arc::new(PluginHandle::load(plugin_path)?);

    if handle.is_active() {
        eprintln!("ljd ingest using active plugin {}", plugin_path.display());
        return run_active_plugin(&handle, spool, next_seq);
    }

    let listener = TcpListener::bind(bind_addr)?;
    eprintln!("ljd ingest listening on {bind_addr} using passive plugin {}", plugin_path.display());

    for stream in listener.incoming() {
        let stream = stream?;
        let peer = stream.peer_addr().ok();
        let handle = Arc::clone(&handle);
        let spool = Arc::clone(&spool);
        let next_seq = Arc::clone(&next_seq);

        thread::Builder::new().name("ljd-plugin-client".to_string()).spawn(move || {
            if let Err(err) = handle_plugin_client(stream, &handle, spool, next_seq) {
                eprintln!("ljd plugin client error: {err}");
            }
            if let Some(peer) = peer {
                eprintln!("ljd plugin client disconnected: {peer}");
            }
        })?;
    }

    Ok(())
}

/// Handles a single TCP client through the plugin parser.
fn handle_plugin_client(
    stream: std::net::TcpStream, handle: &PluginHandle, spool: Arc<super::daemon::SharedSpool>, next_seq: Arc<AtomicU64>,
) -> io::Result<()> {
    let ctx = Box::new(CallbackCtx { spool, next_seq });
    let ctx_ptr = Box::into_raw(ctx) as *mut c_void;

    // SAFETY: we control the lifetime — ctx_ptr stays valid until we Box::from_raw below.
    let plugin_ctx = unsafe { (handle.create)() };
    if plugin_ctx.is_null() {
        // Reclaim the ctx box before returning.
        let _ = unsafe { Box::from_raw(ctx_ptr as *mut CallbackCtx) };
        return Err(io::Error::other("lj_ingest_create returned NULL"));
    }
    unsafe { (handle.set_callback)(plugin_ctx, on_record, ctx_ptr) };

    let mut reader = BufReader::new(stream);
    let mut buf = [0u8; 0x10_000];
    let result = loop {
        match reader.read(&mut buf) {
            Ok(0) => break Ok(()),
            Ok(n) => {
                let rc = unsafe { (handle.feed)(plugin_ctx, buf.as_ptr(), n) };
                if rc != 0 {
                    break Err(io::Error::other(format!("lj_ingest_feed returned error code {rc}")));
                }
            }
            Err(err) => break Err(err),
        }
    };

    unsafe { (handle.free)(plugin_ctx) };
    let _ = unsafe { Box::from_raw(ctx_ptr as *mut CallbackCtx) };
    result
}

/// Runs an active-source plugin that owns its own I/O via `lj_ingest_fetch`.
fn run_active_plugin(handle: &PluginHandle, spool: Arc<super::daemon::SharedSpool>, next_seq: Arc<AtomicU64>) -> io::Result<()> {
    let fetch = handle.fetch.ok_or_else(|| io::Error::other("plugin has no lj_ingest_fetch"))?;

    let ctx = Box::new(CallbackCtx { spool, next_seq });
    let ctx_ptr = Box::into_raw(ctx) as *mut c_void;

    let plugin_ctx = unsafe { (handle.create)() };
    if plugin_ctx.is_null() {
        let _ = unsafe { Box::from_raw(ctx_ptr as *mut CallbackCtx) };
        return Err(io::Error::other("lj_ingest_create returned NULL"));
    }
    unsafe { (handle.set_callback)(plugin_ctx, on_record, ctx_ptr) };

    let rc = unsafe { fetch(plugin_ctx) };

    unsafe { (handle.free)(plugin_ctx) };
    let _ = unsafe { Box::from_raw(ctx_ptr as *mut CallbackCtx) };

    if rc != 0 {
        return Err(io::Error::other(format!("lj_ingest_fetch returned error code {rc}")));
    }
    Ok(())
}
