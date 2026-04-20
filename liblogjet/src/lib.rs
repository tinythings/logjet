//! C/C++ OTLP logging shim and shared exporter ABI for the logjet workspace.

pub mod export;

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::Mutex;
use std::time::Duration;

use opentelemetry_proto::tonic::collector::logs::v1::{ExportLogsServiceRequest, logs_service_client::LogsServiceClient};
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{AnyValue, ArrayValue, InstrumentationScope, KeyValue};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message;
use tokio::runtime::{Builder, Runtime};
use tonic::Request;
use tonic::transport::Endpoint;

const LJ_ATTR_STRING: i32 = 0;
const LJ_ATTR_INT: i32 = 1;
const LJ_ATTR_ARRAY: i32 = 2;
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
    Http(HttpEndpoint),
    Grpc(GrpcClient),
}

#[derive(Clone)]
struct HttpEndpoint {
    authority: String,
    host_header: String,
    path: String,
}

struct GrpcClient {
    endpoint: String,
    runtime: Mutex<Runtime>,
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lj_logger_new_http(endpoint: *const c_char, service_name: *const c_char, timeout_ms: u64) -> *mut lj_logger {
    unsafe { new_logger(endpoint, service_name, timeout_ms, BackendKind::Http) }
}

/// Creates an OTLP/gRPC logger.
///
/// `endpoint` may be either `host:port` or a tonic-compatible URI.
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
    let logger = match unsafe { logger.as_mut() } {
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
    let _ = unsafe { Box::from_raw(logger) };
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
        BackendKind::Http => Backend::Http(parse_http_endpoint(&endpoint)?),
        BackendKind::Grpc => Backend::Grpc(GrpcClient { endpoint: normalise_grpc_endpoint(&endpoint), runtime: Mutex::new(grpc_runtime()?) }),
    };
    Ok(Logger { backend, service_name, timeout })
}

fn grpc_runtime() -> Result<Runtime, String> {
    Builder::new_multi_thread().enable_all().build().map_err(|err| err.to_string())
}

fn build_request(logger: &Logger, record: &LjLogRecord) -> Result<ExportLogsServiceRequest, String> {
    let service_name = read_optional(record.service_name)?.unwrap_or_else(|| logger.service_name.clone());
    if service_name.is_empty() {
        return Err("service name is empty".to_string());
    }

    let severity_text = read_optional(record.severity_text)?.unwrap_or_else(|| severity_text(record.severity_number).to_string());
    let body = read_required_nonnull(record.body, "record.body")?;
    let event_name = read_optional(record.event_name)?.unwrap_or_default();
    let scope_name = read_optional(record.scope_name)?.unwrap_or_else(|| DEFAULT_SCOPE_NAME.to_string());

    let mut resource_attrs = attrs_to_kvs(record.resource_attrs, record.resource_attrs_len)?;
    if !resource_attrs.iter().any(|kv| kv.key == "service.name") {
        resource_attrs.insert(0, key_value("service.name", AnyValue { value: Some(Value::StringValue(service_name.clone())) }));
    }

    let scope_attrs = attrs_to_kvs(record.scope_attrs, record.scope_attrs_len)?;
    let log_attrs = attrs_to_kvs(record.attributes, record.attributes_len)?;

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

    Ok(ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            schema_url: String::new(),
            resource: Some(Resource { attributes: resource_attrs, dropped_attributes_count: 0, entity_refs: Vec::new() }),
            scope_logs: vec![ScopeLogs {
                schema_url: String::new(),
                scope: Some(InstrumentationScope { name: scope_name, version: String::new(), attributes: scope_attrs, dropped_attributes_count: 0 }),
                log_records: vec![log],
            }],
        }],
    })
}

fn attrs_to_kvs(attrs: *const LjAttribute, len: usize) -> Result<Vec<KeyValue>, String> {
    if len == 0 || attrs.is_null() {
        return Ok(Vec::new());
    }
    let attrs = unsafe { std::slice::from_raw_parts(attrs, len) };
    attrs.iter().map(attr_to_kv).collect()
}

fn attr_to_kv(attr: &LjAttribute) -> Result<KeyValue, String> {
    let key = read_required_nonnull(attr.key, "attribute.key")?;
    let raw = read_required_nonnull(attr.value, "attribute.value")?;
    let value = match attr.value_type {
        LJ_ATTR_STRING => AnyValue { value: Some(Value::StringValue(raw)) },
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
    Ok(key_value(key, value))
}

fn key_value(key: impl Into<String>, value: AnyValue) -> KeyValue {
    KeyValue { key: key.into(), value: Some(value) }
}

fn send_request(logger: &Logger, request: ExportLogsServiceRequest) -> Result<(), String> {
    match &logger.backend {
        Backend::Http(endpoint) => post_otlp_http(endpoint, logger.timeout, &request).map_err(|err| err.to_string()),
        Backend::Grpc(client) => client
            .runtime
            .lock()
            .map_err(|_| "gRPC runtime lock poisoned".to_string())?
            .block_on(send_otlp_grpc(client.endpoint.clone(), logger.timeout, request))
            .map_err(|err| err.to_string()),
    }
}

async fn send_otlp_grpc(endpoint: String, timeout: Duration, request: ExportLogsServiceRequest) -> Result<(), Box<dyn std::error::Error>> {
    let channel = Endpoint::from_shared(endpoint)?.timeout(timeout).connect_timeout(timeout).connect().await?;
    let mut client = LogsServiceClient::new(channel);
    client.export(Request::new(request)).await?;
    Ok(())
}

fn post_otlp_http(endpoint: &HttpEndpoint, timeout: Duration, request: &ExportLogsServiceRequest) -> std::io::Result<()> {
    let payload = request.encode_to_vec();
    let mut stream = TcpStream::connect(endpoint.authority.as_str())?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    stream.write_all(
        format!(
            "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/x-protobuf\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            endpoint.path,
            endpoint.host_header,
            payload.len()
        )
        .as_bytes(),
    )?;
    stream.write_all(&payload)?;
    stream.shutdown(Shutdown::Write)?;

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
