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

#[repr(C)]
struct LjAttribute {
    key: *const c_char,
    value: *const c_char,
}

#[repr(C)]
struct LjLogRecord {
    timestamp_unix_ns: u64,
    severity_number: i32,
    severity_text: *const c_char,
    body: *const c_char,
    attributes: *const LjAttribute,
    attributes_len: usize,
}

/// Opaque plugin context. We never dereference it — just pass the pointer.
enum LjIngestPlugin {}

type CreateFn = unsafe extern "C" fn() -> *mut LjIngestPlugin;
type SetCallbackFn = unsafe extern "C" fn(*mut LjIngestPlugin, RecordCallback, *mut c_void);
type FeedFn = unsafe extern "C" fn(*mut LjIngestPlugin, *const u8, usize) -> c_int;
type FreeFn = unsafe extern "C" fn(*mut LjIngestPlugin);
type RecordCallback = unsafe extern "C" fn(*mut c_void, *const LjLogRecord);

// ── Plugin handle ───────────────────────────────────────────────────────────

/// Resolved symbols from a loaded ingest plugin.
struct PluginHandle {
    _lib: libloading::Library,
    create: CreateFn,
    set_callback: SetCallbackFn,
    feed: FeedFn,
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
            let free: libloading::Symbol<FreeFn> =
                lib.get(b"lj_ingest_free\0").map_err(|err| io::Error::other(format!("symbol lj_ingest_free: {err}")))?;

            Ok(Self { create: *create, set_callback: *set_callback, feed: *feed, free: *free, _lib: lib })
        }
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

    let mut attrs = Vec::new();
    if !rec.attributes.is_null() && rec.attributes_len > 0 {
        let slice = unsafe { std::slice::from_raw_parts(rec.attributes, rec.attributes_len) };
        for attr in slice {
            if attr.key.is_null() || attr.value.is_null() {
                continue;
            }
            let key = unsafe { CStr::from_ptr(attr.key) }.to_string_lossy().into_owned();
            let val = unsafe { CStr::from_ptr(attr.value) }.to_string_lossy().into_owned();
            attrs.push((key, val));
        }
    }

    let ts = if rec.timestamp_unix_ns != 0 {
        rec.timestamp_unix_ns
    } else {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos() as u64
    };

    let payload = build_otlp_payload(ts, rec.severity_number, severity_text.as_deref(), &body, &attrs);
    let wire = WireRecord { record_type: logjet::RecordType::Logs, seq: ctx.next_seq.fetch_add(1, Ordering::Relaxed), ts_unix_ns: ts, payload };

    if let Err(err) = super::daemon::append_to_spool(&ctx.spool, wire) {
        eprintln!("ljd plugin callback spool error: {err}");
    }
}

/// Encodes a single log record as an OTLP ExportLogsServiceRequest protobuf.
fn build_otlp_payload(ts: u64, severity: i32, severity_text: Option<&str>, body: &str, attrs: &[(String, String)]) -> Vec<u8> {
    use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
    use opentelemetry_proto::tonic::common::v1::any_value::Value;
    use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope, KeyValue};
    use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
    use opentelemetry_proto::tonic::resource::v1::Resource;
    use prost::Message;

    let record = LogRecord {
        time_unix_nano: ts,
        observed_time_unix_nano: ts,
        severity_number: severity,
        severity_text: severity_text.unwrap_or_default().to_string(),
        body: Some(AnyValue { value: Some(Value::StringValue(body.to_string())) }),
        attributes: attrs
            .iter()
            .map(|(k, v)| KeyValue { key: k.clone(), value: Some(AnyValue { value: Some(Value::StringValue(v.clone())) }) })
            .collect(),
        dropped_attributes_count: 0,
        flags: 0,
        trace_id: Vec::new(),
        span_id: Vec::new(),
        event_name: String::new(),
    };

    let request = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource { attributes: Vec::new(), dropped_attributes_count: 0 }),
            scope_logs: vec![ScopeLogs {
                scope: Some(InstrumentationScope {
                    name: "lj-ingest-plugin".to_string(),
                    version: String::new(),
                    attributes: Vec::new(),
                    dropped_attributes_count: 0,
                }),
                log_records: vec![record],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };

    request.encode_to_vec()
}

// ── Public entry point ──────────────────────────────────────────────────────

/// Runs the plugin ingest loop: loads the .so, binds TCP, feeds bytes.
pub fn plugin_ingest_loop(bind_addr: &str, plugin_path: &Path, spool: Arc<super::daemon::SharedSpool>, next_seq: Arc<AtomicU64>) -> io::Result<()> {
    let handle = Arc::new(PluginHandle::load(plugin_path)?);
    let listener = TcpListener::bind(bind_addr)?;
    eprintln!("ljd ingest listening on {bind_addr} using plugin {}", plugin_path.display());

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
