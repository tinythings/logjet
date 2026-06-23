//! C/C++ OTLP logging shim and shared exporter ABI for the logjet workspace.

pub mod export;

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char};
use std::io::{BufRead, Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use opentelemetry_proto::tonic::collector::logs::v1::{ExportLogsServiceRequest, logs_service_client::LogsServiceClient};
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{AnyValue, ArrayValue, InstrumentationScope, KeyValue};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message;
use tokio::runtime::{Builder, Runtime};
use tokio::sync::{Mutex as TokioMutex, Notify, OwnedSemaphorePermit, Semaphore};
use tonic::Request;
use tonic::transport::{Channel, Endpoint};

const LJ_ATTR_STRING: i32 = 0;
const LJ_ATTR_INT: i32 = 1;
const LJ_ATTR_ARRAY: i32 = 2;
const LJ_BACKPRESSURE_UNBOUNDED: i32 = 0;
const LJ_BACKPRESSURE_DROP: i32 = 1;
const LJ_BACKPRESSURE_BLOCK: i32 = 2;
const DEFAULT_BACKPRESSURE_CAPACITY: usize = 1024;
const MAX_HTTP_POOL: usize = 256;
const DEFAULT_SCOPE_NAME: &str = "liblogjet";
const VERSION: &[u8] = concat!(env!("CARGO_PKG_VERSION"), "\0").as_bytes();

thread_local! {
    static LAST_ERROR: RefCell<CString> = RefCell::new(CString::new("").expect("static"));
}

/// OpenTelemetry-compatible string attribute.
#[repr(C)]
pub struct LjAttribute {
    /// Attribute key.
    pub key: *const c_char,
    /// Attribute value as UTF-8 text.
    pub value: *const c_char,
    /// Value kind: string, int, or array.
    pub value_type: i32,
}

/// FFI log record accepted by `lj_logger_log`.
#[repr(C)]
pub struct LjLogRecord {
    /// Unix timestamp in nanoseconds.
    pub timestamp_unix_ns: u64,
    /// OpenTelemetry severity number.
    pub severity_number: i32,
    /// Optional severity text. Falls back to a derived label when null.
    pub severity_text: *const c_char,
    /// Record body text.
    pub body: *const c_char,
    /// Record-level attributes.
    pub attributes: *const LjAttribute,
    /// Number of record-level attributes.
    pub attributes_len: usize,
    /// Optional event name.
    pub event_name: *const c_char,
    /// Optional per-record service name override.
    pub service_name: *const c_char,
    /// Optional instrumentation scope name.
    pub scope_name: *const c_char,
    /// Optional resource attributes.
    pub resource_attrs: *const LjAttribute,
    /// Number of resource attributes.
    pub resource_attrs_len: usize,
    /// Optional instrumentation scope attributes.
    pub scope_attrs: *const LjAttribute,
    /// Number of instrumentation scope attributes.
    pub scope_attrs_len: usize,
}

/// Opaque logger handle owned by the shared library.
#[repr(C)]
pub struct lj_logger {
    inner: Logger,
}

struct Logger {
    backend: Backend,
    service_name: String,
    timeout: Duration,
}

enum Backend {
    Http(HttpClient),
    Grpc(GrpcClient),
}

#[derive(Clone)]
struct HttpEndpoint {
    authority: String,
    host_header: String,
    path: String,
}

struct GrpcClient {
    runtime: Runtime,
    engine: Arc<AsyncEngine>,
    channel: Arc<GrpcChannel>,
}

struct GrpcChannel {
    endpoint: String,
    channel: TokioMutex<Option<Channel>>,
}

struct HttpClient {
    runtime: OnceLock<Runtime>,
    engine: Arc<AsyncEngine>,
    pool: Arc<HttpPool>,
}

struct HttpPool {
    endpoint: HttpEndpoint,
    idle: Mutex<Vec<TcpStream>>,
}

/// Backend-agnostic async send engine: backpressure, counters, and drain.
struct AsyncEngine {
    backpressure: Mutex<Backpressure>,
    inflight: AtomicU64,
    errors: AtomicU64,
    dropped: AtomicU64,
    idle: Notify,
}

impl AsyncEngine {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            backpressure: Mutex::new(Backpressure { model: LJ_BACKPRESSURE_DROP, semaphore: Arc::new(Semaphore::new(DEFAULT_BACKPRESSURE_CAPACITY)) }),
            inflight: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            idle: Notify::new(),
        })
    }
}

struct Backpressure {
    model: i32,
    semaphore: Arc<Semaphore>,
}

/// Returns the library version string.
#[unsafe(no_mangle)]
pub extern "C" fn lj_version() -> *const c_char {
    VERSION.as_ptr().cast::<c_char>()
}

/// Returns the calling thread's last error message.
#[unsafe(no_mangle)]
pub extern "C" fn lj_error_message() -> *const c_char {
    LAST_ERROR.with(|slot| slot.borrow().as_ptr())
}

/// Creates an OTLP/HTTP logger.
///
/// `endpoint` may be either `host:port` or `http://host:port[/path]`.
///
/// # Safety
///
/// `endpoint` and `service_name` must be valid pointers to NUL-terminated C
/// strings for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lj_logger_new_http(endpoint: *const c_char, service_name: *const c_char, timeout_ms: u64) -> *mut lj_logger {
    unsafe { new_logger(endpoint, service_name, timeout_ms, BackendKind::Http) }
}

/// Creates an OTLP/gRPC logger.
///
/// `endpoint` may be either `host:port` or a tonic-compatible URI.
///
/// # Safety
///
/// `endpoint` and `service_name` must be valid pointers to NUL-terminated C
/// strings for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lj_logger_new_grpc(endpoint: *const c_char, service_name: *const c_char, timeout_ms: u64) -> *mut lj_logger {
    unsafe { new_logger(endpoint, service_name, timeout_ms, BackendKind::Grpc) }
}

/// Sends one log record.
///
/// # Safety
///
/// `logger` must be a valid pointer returned by `lj_logger_new_http` or
/// `lj_logger_new_grpc`. `record` must point to a valid `LjLogRecord` for the
/// duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lj_logger_log(logger: *mut lj_logger, record: *const LjLogRecord) -> bool {
    clear_last_error();
    let logger = match unsafe { logger.as_ref() } {
        Some(logger) => logger,
        None => {
            set_last_error("logger is null");
            return false;
        }
    };
    let record = match unsafe { record.as_ref() } {
        Some(record) => record,
        None => {
            set_last_error("record is null");
            return false;
        }
    };
    match build_request(&logger.inner, record).and_then(|request| send_request(&logger.inner, request)) {
        Ok(()) => true,
        Err(err) => {
            set_last_error(err);
            false
        }
    }
}

/// Sends one log record, reusing a persistent connection.
///
/// For gRPC loggers this reuses a cached channel, avoiding a fresh connection
/// handshake on every call. For HTTP loggers it currently behaves like
/// `lj_logger_log` (a new connection per call) until HTTP keep-alive lands.
///
/// # Safety
///
/// `logger` must be a valid pointer returned by `lj_logger_new_http` or
/// `lj_logger_new_grpc`. `record` must point to a valid `LjLogRecord` for the
/// duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lj_logger_log_reuse(logger: *mut lj_logger, record: *const LjLogRecord) -> bool {
    clear_last_error();
    let logger = match unsafe { logger.as_ref() } {
        Some(logger) => logger,
        None => {
            set_last_error("logger is null");
            return false;
        }
    };
    let record = match unsafe { record.as_ref() } {
        Some(record) => record,
        None => {
            set_last_error("record is null");
            return false;
        }
    };
    match build_request(&logger.inner, record).and_then(|request| send_request_reuse(&logger.inner, request)) {
        Ok(()) => true,
        Err(err) => {
            set_last_error(err);
            false
        }
    }
}

/// Sends a batch of records in a single export request over a persistent
/// connection.
///
/// Records sharing the same effective service name, scope name, resource
/// attributes, and scope attributes are grouped into one `ScopeLogs`. A `len`
/// of `0` (or a null `records` pointer) is a successful no-op. For gRPC loggers
/// the batch is sent over the reused channel; HTTP loggers send one POST for
/// the whole batch.
///
/// # Safety
///
/// `logger` must be a valid pointer returned by `lj_logger_new_http` or
/// `lj_logger_new_grpc`. When `len > 0`, `records` must point to `len` valid
/// `LjLogRecord` values for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lj_logger_log_batch(logger: *mut lj_logger, records: *const LjLogRecord, len: usize) -> bool {
    clear_last_error();
    let logger = match unsafe { logger.as_ref() } {
        Some(logger) => logger,
        None => {
            set_last_error("logger is null");
            return false;
        }
    };
    if len == 0 || records.is_null() {
        return true;
    }
    let records = unsafe { std::slice::from_raw_parts(records, len) };
    match build_batch_request(&logger.inner, records).and_then(|request| send_request_reuse(&logger.inner, request)) {
        Ok(()) => true,
        Err(err) => {
            set_last_error(err);
            false
        }
    }
}

/// Sends one log record without blocking, handing the send to a background
/// runtime and returning immediately.
///
/// gRPC only. Returns `true` if the record was validated and enqueued; `false`
/// (with `lj_error_message` set) only on immediate validation errors. Network
/// failures occur later and are counted by `lj_logger_async_errors`; records
/// dropped by backpressure are counted by `lj_logger_async_dropped`.
///
/// # Safety
///
/// `logger` must be a valid pointer returned by `lj_logger_new_grpc`. `record`
/// must point to a valid `LjLogRecord` for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lj_logger_log_async(logger: *mut lj_logger, record: *const LjLogRecord) -> bool {
    clear_last_error();
    let logger = match unsafe { logger.as_ref() } {
        Some(logger) => logger,
        None => {
            set_last_error("logger is null");
            return false;
        }
    };
    let record = match unsafe { record.as_ref() } {
        Some(record) => record,
        None => {
            set_last_error("record is null");
            return false;
        }
    };
    match build_request(&logger.inner, record).and_then(|request| send_request_async(&logger.inner, request)) {
        Ok(()) => true,
        Err(err) => {
            set_last_error(err);
            false
        }
    }
}

/// Sends a batch of records without blocking (gRPC only). A `len` of `0` (or a
/// null `records` pointer) is a successful no-op.
///
/// # Safety
///
/// `logger` must be a valid pointer returned by `lj_logger_new_grpc`. When
/// `len > 0`, `records` must point to `len` valid `LjLogRecord` values for the
/// duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lj_logger_log_batch_async(logger: *mut lj_logger, records: *const LjLogRecord, len: usize) -> bool {
    clear_last_error();
    let logger = match unsafe { logger.as_ref() } {
        Some(logger) => logger,
        None => {
            set_last_error("logger is null");
            return false;
        }
    };
    if len == 0 || records.is_null() {
        return true;
    }
    let records = unsafe { std::slice::from_raw_parts(records, len) };
    match build_batch_request(&logger.inner, records).and_then(|request| send_request_async(&logger.inner, request)) {
        Ok(()) => true,
        Err(err) => {
            set_last_error(err);
            false
        }
    }
}

fn logger_engine(logger: &Logger) -> &Arc<AsyncEngine> {
    match &logger.backend {
        Backend::Grpc(client) => &client.engine,
        Backend::Http(client) => &client.engine,
    }
}

fn flush_logger(logger: &Logger, timeout: Duration) -> bool {
    match &logger.backend {
        Backend::Grpc(client) => flush_engine(&client.runtime, &client.engine, timeout),
        Backend::Http(client) => match client.runtime.get() {
            Some(runtime) => flush_engine(runtime, &client.engine, timeout),
            None => client.engine.inflight.load(Ordering::SeqCst) == 0,
        },
    }
}

/// Configures the async backpressure policy (gRPC or HTTP).
///
/// `model` is one of `LJ_BACKPRESSURE_UNBOUNDED`, `LJ_BACKPRESSURE_DROP`, or
/// `LJ_BACKPRESSURE_BLOCK`. `capacity` is the maximum number of in-flight async
/// sends for the bounded models (ignored for unbounded). Should be called
/// before the first async send. Returns `false` (with `lj_error_message`) on
/// invalid input.
///
/// # Safety
///
/// `logger` must be a valid pointer returned by this library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lj_logger_set_backpressure(logger: *mut lj_logger, model: i32, capacity: usize) -> bool {
    clear_last_error();
    let logger = match unsafe { logger.as_ref() } {
        Some(logger) => logger,
        None => {
            set_last_error("logger is null");
            return false;
        }
    };
    if model != LJ_BACKPRESSURE_UNBOUNDED && model != LJ_BACKPRESSURE_DROP && model != LJ_BACKPRESSURE_BLOCK {
        set_last_error("invalid backpressure model");
        return false;
    }
    let capacity = if model == LJ_BACKPRESSURE_UNBOUNDED {
        1
    } else if capacity == 0 {
        set_last_error("capacity must be >= 1 for bounded backpressure");
        return false;
    } else {
        capacity
    };
    match logger_engine(&logger.inner).backpressure.lock() {
        Ok(mut cfg) => {
            cfg.model = model;
            cfg.semaphore = Arc::new(Semaphore::new(capacity));
            true
        }
        Err(_) => {
            set_last_error("backpressure lock poisoned");
            false
        }
    }
}

/// Blocks until all in-flight async sends complete or `timeout_ms` elapses.
///
/// Returns `true` if fully drained, `false` on timeout.
///
/// # Safety
///
/// `logger` must be a valid pointer returned by this library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lj_logger_flush(logger: *mut lj_logger, timeout_ms: u64) -> bool {
    let logger = match unsafe { logger.as_ref() } {
        Some(logger) => logger,
        None => return false,
    };
    flush_logger(&logger.inner, Duration::from_millis(timeout_ms))
}

/// Returns the number of async sends that failed on the network.
///
/// # Safety
///
/// `logger` must be null or a valid pointer returned by this library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lj_logger_async_errors(logger: *mut lj_logger) -> u64 {
    match unsafe { logger.as_ref() } {
        Some(logger) => logger_engine(&logger.inner).errors.load(Ordering::Relaxed),
        None => 0,
    }
}

/// Returns the number of records dropped by bounded backpressure.
///
/// # Safety
///
/// `logger` must be null or a valid pointer returned by this library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lj_logger_async_dropped(logger: *mut lj_logger) -> u64 {
    match unsafe { logger.as_ref() } {
        Some(logger) => logger_engine(&logger.inner).dropped.load(Ordering::Relaxed),
        None => 0,
    }
}

/// Returns the number of async sends currently in flight.
///
/// # Safety
///
/// `logger` must be null or a valid pointer returned by this library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lj_logger_async_inflight(logger: *mut lj_logger) -> u64 {
    match unsafe { logger.as_ref() } {
        Some(logger) => logger_engine(&logger.inner).inflight.load(Ordering::SeqCst),
        None => 0,
    }
}

/// Frees a logger created by one of the constructors. Accepts null.
///
/// # Safety
///
/// `logger` must be null or a valid pointer returned by this library and not
/// already freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lj_logger_free(logger: *mut lj_logger) {
    if logger.is_null() {
        return;
    }
    let boxed = unsafe { Box::from_raw(logger) };
    let _ = flush_logger(&boxed.inner, boxed.inner.timeout);
}

enum BackendKind {
    Http,
    Grpc,
}

unsafe fn new_logger(endpoint: *const c_char, service_name: *const c_char, timeout_ms: u64, kind: BackendKind) -> *mut lj_logger {
    clear_last_error();
    match unsafe { new_logger_impl(endpoint, service_name, timeout_ms, kind) } {
        Ok(logger) => Box::into_raw(Box::new(lj_logger { inner: logger })),
        Err(err) => {
            set_last_error(err);
            std::ptr::null_mut()
        }
    }
}

unsafe fn new_logger_impl(endpoint: *const c_char, service_name: *const c_char, timeout_ms: u64, kind: BackendKind) -> Result<Logger, String> {
    let endpoint = read_required(endpoint, "endpoint")?;
    let service_name = read_required(service_name, "service_name")?;
    let timeout = Duration::from_millis(timeout_ms.max(1));
    let backend = match kind {
        BackendKind::Http => {
            let pool = Arc::new(HttpPool { endpoint: parse_http_endpoint(&endpoint)?, idle: Mutex::new(Vec::new()) });
            Backend::Http(HttpClient { runtime: OnceLock::new(), engine: AsyncEngine::new(), pool })
        }
        BackendKind::Grpc => {
            let channel = Arc::new(GrpcChannel { endpoint: normalise_grpc_endpoint(&endpoint), channel: TokioMutex::new(None) });
            Backend::Grpc(GrpcClient { runtime: make_runtime()?, engine: AsyncEngine::new(), channel })
        }
    };
    Ok(Logger { backend, service_name, timeout })
}

fn make_runtime() -> Result<Runtime, String> {
    Builder::new_multi_thread().enable_all().build().map_err(|err| err.to_string())
}

/// Raw `(key, value_type, value)` attribute triples read once from the C side.
type AttrTriples = Vec<(String, i32, String)>;
/// Grouping key for a resource: effective service name plus resource attributes.
type ResourceKey = (String, AttrTriples);
/// Grouping key for a scope: effective scope name plus scope attributes.
type ScopeKey = (String, AttrTriples);

fn build_request(logger: &Logger, record: &LjLogRecord) -> Result<ExportLogsServiceRequest, String> {
    let (_, resource_attrs) = resolve_resource(logger, record)?;
    let (_, scope) = resolve_scope(record)?;
    let log = record_to_log(record)?;

    Ok(ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            schema_url: String::new(),
            resource: Some(Resource { attributes: resource_attrs, dropped_attributes_count: 0, entity_refs: Vec::new() }),
            scope_logs: vec![ScopeLogs { schema_url: String::new(), scope: Some(scope), log_records: vec![log] }],
        }],
    })
}

fn build_batch_request(logger: &Logger, records: &[LjLogRecord]) -> Result<ExportLogsServiceRequest, String> {
    use std::collections::HashMap;

    struct ScopeGroup {
        scope: InstrumentationScope,
        log_records: Vec<LogRecord>,
    }
    struct ResourceGroup {
        resource_attrs: Vec<KeyValue>,
        scopes: Vec<ScopeGroup>,
        scope_index: HashMap<ScopeKey, usize>,
    }

    let mut groups: Vec<ResourceGroup> = Vec::new();
    let mut resource_index: HashMap<ResourceKey, usize> = HashMap::new();

    for record in records {
        let (resource_key, resource_attrs) = resolve_resource(logger, record)?;
        let (scope_key, scope) = resolve_scope(record)?;
        let log = record_to_log(record)?;

        let resource_idx = match resource_index.get(&resource_key) {
            Some(idx) => *idx,
            None => {
                groups.push(ResourceGroup { resource_attrs, scopes: Vec::new(), scope_index: HashMap::new() });
                let idx = groups.len() - 1;
                resource_index.insert(resource_key, idx);
                idx
            }
        };

        let group = &mut groups[resource_idx];
        let scope_idx = match group.scope_index.get(&scope_key) {
            Some(idx) => *idx,
            None => {
                group.scopes.push(ScopeGroup { scope, log_records: Vec::new() });
                let idx = group.scopes.len() - 1;
                group.scope_index.insert(scope_key, idx);
                idx
            }
        };
        group.scopes[scope_idx].log_records.push(log);
    }

    Ok(ExportLogsServiceRequest {
        resource_logs: groups
            .into_iter()
            .map(|group| ResourceLogs {
                schema_url: String::new(),
                resource: Some(Resource { attributes: group.resource_attrs, dropped_attributes_count: 0, entity_refs: Vec::new() }),
                scope_logs: group
                    .scopes
                    .into_iter()
                    .map(|scope_group| ScopeLogs { schema_url: String::new(), scope: Some(scope_group.scope), log_records: scope_group.log_records })
                    .collect(),
            })
            .collect(),
    })
}

fn resolve_resource(logger: &Logger, record: &LjLogRecord) -> Result<(ResourceKey, Vec<KeyValue>), String> {
    let service_name = read_optional(record.service_name)?.unwrap_or_else(|| logger.service_name.clone());
    if service_name.is_empty() {
        return Err("service name is empty".to_string());
    }

    let triples = read_attrs(record.resource_attrs, record.resource_attrs_len)?;
    let mut resource_attrs = triples_to_kvs(&triples)?;
    if !triples.iter().any(|(key, _, _)| key == "service.name") {
        resource_attrs.insert(0, key_value("service.name", AnyValue { value: Some(Value::StringValue(service_name.clone())) }));
    }

    Ok(((service_name, triples), resource_attrs))
}

fn resolve_scope(record: &LjLogRecord) -> Result<(ScopeKey, InstrumentationScope), String> {
    let scope_name = read_optional(record.scope_name)?.unwrap_or_else(|| DEFAULT_SCOPE_NAME.to_string());
    let triples = read_attrs(record.scope_attrs, record.scope_attrs_len)?;
    let scope_attrs = triples_to_kvs(&triples)?;
    let scope = InstrumentationScope { name: scope_name.clone(), version: String::new(), attributes: scope_attrs, dropped_attributes_count: 0 };
    Ok(((scope_name, triples), scope))
}

fn record_to_log(record: &LjLogRecord) -> Result<LogRecord, String> {
    let severity_text = read_optional(record.severity_text)?.unwrap_or_else(|| severity_text(record.severity_number).to_string());
    let body = read_required_nonnull(record.body, "record.body")?;
    let event_name = read_optional(record.event_name)?.unwrap_or_default();
    let log_attrs = triples_to_kvs(&read_attrs(record.attributes, record.attributes_len)?)?;

    let mut log = LogRecord {
        time_unix_nano: record.timestamp_unix_ns,
        observed_time_unix_nano: 0,
        severity_number: record.severity_number,
        severity_text,
        body: Some(AnyValue { value: Some(Value::StringValue(body)) }),
        attributes: log_attrs,
        dropped_attributes_count: 0,
        flags: 0,
        trace_id: Vec::new(),
        span_id: Vec::new(),
        event_name,
    };
    if log.time_unix_nano == 0 {
        log.time_unix_nano = now_unix_ns();
    }
    Ok(log)
}

fn read_attrs(attrs: *const LjAttribute, len: usize) -> Result<AttrTriples, String> {
    if len == 0 || attrs.is_null() {
        return Ok(Vec::new());
    }
    let attrs = unsafe { std::slice::from_raw_parts(attrs, len) };
    attrs
        .iter()
        .map(|attr| {
            let key = read_required_nonnull(attr.key, "attribute.key")?;
            let value = read_required_nonnull(attr.value, "attribute.value")?;
            Ok((key, attr.value_type, value))
        })
        .collect()
}

fn triples_to_kvs(triples: &[(String, i32, String)]) -> Result<Vec<KeyValue>, String> {
    triples
        .iter()
        .map(|(key, value_type, raw)| {
            let value = match *value_type {
                LJ_ATTR_STRING => AnyValue { value: Some(Value::StringValue(raw.clone())) },
                LJ_ATTR_INT => AnyValue { value: Some(Value::IntValue(raw.parse::<i64>().unwrap_or(0))) },
                LJ_ATTR_ARRAY => AnyValue {
                    value: Some(Value::ArrayValue(ArrayValue {
                        values: raw
                            .split(',')
                            .map(str::trim)
                            .filter(|part| !part.is_empty())
                            .map(|part| AnyValue { value: Some(Value::StringValue(part.to_string())) })
                            .collect(),
                    })),
                },
                other => return Err(format!("unsupported attribute value_type {other}")),
            };
            Ok(key_value(key.clone(), value))
        })
        .collect()
}

fn key_value(key: impl Into<String>, value: AnyValue) -> KeyValue {
    KeyValue { key: key.into(), value: Some(value) }
}

fn send_request(logger: &Logger, request: ExportLogsServiceRequest) -> Result<(), String> {
    match &logger.backend {
        Backend::Http(client) => post_otlp_http_once(&client.pool.endpoint, logger.timeout, &request).map_err(|err| err.to_string()),
        Backend::Grpc(client) => {
            client.runtime.block_on(send_otlp_grpc(client.channel.endpoint.clone(), logger.timeout, request)).map_err(|err| err.to_string())
        }
    }
}

fn send_request_reuse(logger: &Logger, request: ExportLogsServiceRequest) -> Result<(), String> {
    match &logger.backend {
        Backend::Http(client) => {
            let payload = request.encode_to_vec();
            http_send_blocking(&client.pool, logger.timeout, &payload).map_err(|err| err.to_string())
        }
        Backend::Grpc(client) => client.runtime.block_on(send_pooled_async(client.channel.clone(), logger.timeout, request)),
    }
}

fn send_request_async(logger: &Logger, request: ExportLogsServiceRequest) -> Result<(), String> {
    match &logger.backend {
        Backend::Grpc(client) => {
            let channel = client.channel.clone();
            let timeout = logger.timeout;
            enqueue_async(&client.runtime, &client.engine, move || send_pooled_async(channel, timeout, request))
        }
        Backend::Http(client) => {
            let runtime = http_runtime(client)?;
            let pool = client.pool.clone();
            let timeout = logger.timeout;
            let payload = request.encode_to_vec();
            enqueue_async(runtime, &client.engine, move || http_send_async(pool, timeout, payload))
        }
    }
}

fn http_runtime(client: &HttpClient) -> Result<&Runtime, String> {
    if let Some(runtime) = client.runtime.get() {
        return Ok(runtime);
    }
    let runtime = make_runtime()?;
    let _ = client.runtime.set(runtime);
    Ok(client.runtime.get().expect("http runtime set"))
}

fn enqueue_async<F, Fut>(runtime: &Runtime, engine: &Arc<AsyncEngine>, make_fut: F) -> Result<(), String>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<(), String>> + Send + 'static,
{
    let (model, semaphore) = {
        let cfg = engine.backpressure.lock().map_err(|_| "backpressure lock poisoned".to_string())?;
        (cfg.model, cfg.semaphore.clone())
    };
    match model {
        LJ_BACKPRESSURE_UNBOUNDED => spawn_task(runtime, engine, make_fut, None),
        LJ_BACKPRESSURE_DROP => match semaphore.try_acquire_owned() {
            Ok(permit) => spawn_task(runtime, engine, make_fut, Some(permit)),
            Err(_) => {
                engine.dropped.fetch_add(1, Ordering::Relaxed);
            }
        },
        LJ_BACKPRESSURE_BLOCK => {
            let permit = runtime.block_on(semaphore.acquire_owned()).map_err(|err| err.to_string())?;
            spawn_task(runtime, engine, make_fut, Some(permit));
        }
        other => return Err(format!("invalid backpressure model {other}")),
    }
    Ok(())
}

fn spawn_task<F, Fut>(runtime: &Runtime, engine: &Arc<AsyncEngine>, make_fut: F, permit: Option<OwnedSemaphorePermit>)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<(), String>> + Send + 'static,
{
    let engine = engine.clone();
    engine.inflight.fetch_add(1, Ordering::SeqCst);
    runtime.spawn(async move {
        let _permit = permit;
        if make_fut().await.is_err() {
            engine.errors.fetch_add(1, Ordering::Relaxed);
        }
        if engine.inflight.fetch_sub(1, Ordering::SeqCst) == 1 {
            engine.idle.notify_waiters();
        }
    });
}

fn flush_engine(runtime: &Runtime, engine: &Arc<AsyncEngine>, timeout: Duration) -> bool {
    runtime.block_on(async {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if engine.inflight.load(Ordering::SeqCst) == 0 {
                return true;
            }
            let notified = engine.idle.notified();
            if engine.inflight.load(Ordering::SeqCst) == 0 {
                return true;
            }
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return false;
            }
            if tokio::time::timeout(deadline - now, notified).await.is_err() {
                return engine.inflight.load(Ordering::SeqCst) == 0;
            }
        }
    })
}

async fn send_pooled_async(channel: Arc<GrpcChannel>, timeout: Duration, request: ExportLogsServiceRequest) -> Result<(), String> {
    // Connect-once: hold the lock across connect so a concurrent burst opens a
    // single connection; release it before export so sends run concurrently over
    // the shared multiplexed channel.
    let active = {
        let mut guard = channel.channel.lock().await;
        match guard.as_ref() {
            Some(active) => active.clone(),
            None => {
                let active = connect_channel(&channel.endpoint, timeout).await.map_err(|err| err.to_string())?;
                *guard = Some(active.clone());
                active
            }
        }
    };

    let result = export_on_channel(active, request).await.map_err(|err| err.to_string());
    if result.is_err() {
        *channel.channel.lock().await = None;
    }
    result
}

async fn connect_channel(endpoint: &str, timeout: Duration) -> Result<Channel, Box<dyn std::error::Error>> {
    Ok(Endpoint::from_shared(endpoint.to_string())?.timeout(timeout).connect_timeout(timeout).connect().await?)
}

async fn export_on_channel(channel: Channel, request: ExportLogsServiceRequest) -> Result<(), Box<dyn std::error::Error>> {
    let mut client = LogsServiceClient::new(channel);
    client.export(Request::new(request)).await?;
    Ok(())
}

async fn send_otlp_grpc(endpoint: String, timeout: Duration, request: ExportLogsServiceRequest) -> Result<(), Box<dyn std::error::Error>> {
    let channel = Endpoint::from_shared(endpoint)?.timeout(timeout).connect_timeout(timeout).connect().await?;
    let mut client = LogsServiceClient::new(channel);
    client.export(Request::new(request)).await?;
    Ok(())
}

/// Fresh-connect, `Connection: close` POST. Baseline used by `lj_logger_log`.
fn post_otlp_http_once(endpoint: &HttpEndpoint, timeout: Duration, request: &ExportLogsServiceRequest) -> std::io::Result<()> {
    let payload = request.encode_to_vec();
    let mut stream = TcpStream::connect(endpoint.authority.as_str())?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    stream.set_nodelay(true)?;
    let header = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/x-protobuf\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        endpoint.path,
        endpoint.host_header,
        payload.len()
    );
    let mut framed = Vec::with_capacity(header.len() + payload.len());
    framed.extend_from_slice(header.as_bytes());
    framed.extend_from_slice(&payload);
    stream.write_all(&framed)?;

    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    let head_end = response.windows(4).position(|chunk| chunk == b"\r\n\r\n").map(|idx| idx + 4).unwrap_or(response.len());
    let head = String::from_utf8_lossy(&response[..head_end]);
    let status = head.lines().next().unwrap_or_default();
    if status.contains(" 200 ") || status.contains(" 202 ") {
        return Ok(());
    }
    Err(std::io::Error::other(format!("HTTP export failed: {status}")))
}

async fn http_send_async(pool: Arc<HttpPool>, timeout: Duration, payload: Vec<u8>) -> Result<(), String> {
    tokio::task::spawn_blocking(move || http_send_blocking(&pool, timeout, &payload))
        .await
        .map_err(|err| err.to_string())?
        .map_err(|err| err.to_string())
}

/// Keep-alive POST over a pooled connection, with one fresh-connect retry.
fn http_send_blocking(pool: &HttpPool, timeout: Duration, payload: &[u8]) -> std::io::Result<()> {
    if let Some(stream) = pool_checkout(pool)
        && http_exchange(pool, stream, payload).is_ok()
    {
        return Ok(());
    }
    let stream = http_connect(pool, timeout)?;
    http_exchange(pool, stream, payload)
}

fn pool_checkout(pool: &HttpPool) -> Option<TcpStream> {
    pool.idle.lock().ok().and_then(|mut idle| idle.pop())
}

fn pool_checkin(pool: &HttpPool, stream: TcpStream) {
    if let Ok(mut idle) = pool.idle.lock()
        && idle.len() < MAX_HTTP_POOL
    {
        idle.push(stream);
    }
}

fn http_connect(pool: &HttpPool, timeout: Duration) -> std::io::Result<TcpStream> {
    let stream = TcpStream::connect(pool.endpoint.authority.as_str())?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    stream.set_nodelay(true)?;
    Ok(stream)
}

fn http_exchange(pool: &HttpPool, mut stream: TcpStream, payload: &[u8]) -> std::io::Result<()> {
    let header = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/x-protobuf\r\nContent-Length: {}\r\n\r\n",
        pool.endpoint.path,
        pool.endpoint.host_header,
        payload.len()
    );
    // Single write (header + body) so Nagle/delayed-ACK does not stall the request.
    let mut framed = Vec::with_capacity(header.len() + payload.len());
    framed.extend_from_slice(header.as_bytes());
    framed.extend_from_slice(payload);
    stream.write_all(&framed)?;
    stream.flush()?;

    let (status_ok, keep_alive) = read_http_response(&mut stream)?;
    if !status_ok {
        return Err(std::io::Error::other("HTTP export failed"));
    }
    if keep_alive {
        pool_checkin(pool, stream);
    }
    Ok(())
}

/// Reads one HTTP/1.1 response. Returns `(status_ok, can_keep_alive)`.
fn read_http_response(stream: &mut TcpStream) -> std::io::Result<(bool, bool)> {
    let mut reader = std::io::BufReader::new(stream);

    let mut head = String::new();
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line)?;
        if read == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        head.push_str(&line);
    }

    let status_line = head.lines().next().unwrap_or_default();
    let status_ok = status_line.contains(" 200 ") || status_line.contains(" 202 ") || status_line.contains(" 204 ");

    let mut content_length: Option<usize> = None;
    let mut conn_close = false;
    for line in head.lines().skip(1) {
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim();
            if key == "content-length" {
                content_length = value.parse::<usize>().ok();
            } else if key == "connection" && value.eq_ignore_ascii_case("close") {
                conn_close = true;
            }
        }
    }

    let keep_alive = match content_length {
        Some(len) => {
            let mut body = vec![0u8; len];
            reader.read_exact(&mut body)?;
            !conn_close
        }
        None => {
            let mut sink = Vec::new();
            let _ = reader.read_to_end(&mut sink);
            false
        }
    };

    Ok((status_ok, keep_alive))
}

fn parse_http_endpoint(raw: &str) -> Result<HttpEndpoint, String> {
    let raw = raw.trim();
    if let Some(rest) = raw.strip_prefix("https://") {
        return Err(format!("TLS is not supported yet for OTLP/HTTP endpoint `{rest}`"));
    }
    let rest = raw.strip_prefix("http://").unwrap_or(raw);
    let (authority, path) = match rest.split_once('/') {
        Some((host, tail)) => (host, format!("/{}", tail.trim_start_matches('/'))),
        None => (rest, "/v1/logs".to_string()),
    };
    if authority.is_empty() {
        return Err("HTTP endpoint is empty".to_string());
    }
    Ok(HttpEndpoint { authority: authority.to_string(), host_header: authority.to_string(), path })
}

fn normalise_grpc_endpoint(raw: &str) -> String {
    let raw = raw.trim();
    if raw.contains("://") { raw.to_string() } else { format!("http://{raw}") }
}

fn read_required(ptr: *const c_char, name: &str) -> Result<String, String> {
    let value = read_required_nonnull(ptr, name)?;
    if value.is_empty() { Err(format!("{name} is empty")) } else { Ok(value) }
}

fn read_required_nonnull(ptr: *const c_char, name: &str) -> Result<String, String> {
    if ptr.is_null() {
        return Err(format!("{name} is null"));
    }
    read_c_string(ptr, name)
}

fn read_optional(ptr: *const c_char) -> Result<Option<String>, String> {
    if ptr.is_null() { Ok(None) } else { read_c_string(ptr, "string").map(Some) }
}

fn read_c_string(ptr: *const c_char, name: &str) -> Result<String, String> {
    unsafe { CStr::from_ptr(ptr) }.to_str().map(|value| value.to_string()).map_err(|err| format!("{name} is not valid UTF-8: {err}"))
}

fn severity_text(number: i32) -> &'static str {
    match number {
        i32::MIN..=4 => "TRACE",
        5..=8 => "DEBUG",
        9..=12 => "INFO",
        13..=16 => "WARN",
        17..=20 => "ERROR",
        _ => "FATAL",
    }
}

fn now_unix_ns() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos() as u64
}

fn clear_last_error() {
    set_last_error("");
}

fn set_last_error(message: impl Into<String>) {
    let clean = message.into().replace('\0', " ");
    LAST_ERROR.with(|slot| {
        *slot.borrow_mut() = CString::new(clean).unwrap_or_else(|_| CString::new("invalid error").expect("static"));
    });
}

#[cfg(test)]
#[path = "../tests/unit/batch_ut.rs"]
mod batch_ut;
