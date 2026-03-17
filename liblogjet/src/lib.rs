use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char};
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::ptr;
use std::time::Duration;

use opentelemetry_proto::tonic::collector::logs::v1::{ExportLogsServiceRequest, logs_service_client::LogsServiceClient};
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope, KeyValue};
use opentelemetry_proto::tonic::logs::v1::LogRecord;
use opentelemetry_proto::tonic::logs::v1::SeverityNumber;
use opentelemetry_proto::tonic::logs::v1::{ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message;
use tokio::runtime::Runtime;
use tonic::Request;

thread_local! {
    static LAST_ERROR: RefCell<CString> = RefCell::new(cstring_lossy("ok"));
}

#[repr(C)]
pub struct lj_attribute {
    key: *const c_char,
    value: *const c_char,
}

#[repr(C)]
pub struct lj_log_record {
    timestamp_unix_ns: u64,
    severity_number: i32,
    severity_text: *const c_char,
    body: *const c_char,
    attributes: *const lj_attribute,
    attributes_len: usize,
}

pub struct LjLogger {
    transport: Transport,
    service_name: String,
    timeout: Duration,
}

struct LogRecordInput {
    timestamp_unix_ns: u64,
    severity_number: i32,
    severity_text: Option<String>,
    body: String,
    attributes: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
struct HttpEndpoint {
    authority: String,
    path: String,
}

#[derive(Debug, Clone)]
struct GrpcEndpoint {
    url: String,
}

enum Transport {
    Http(HttpEndpoint),
    Grpc { endpoint: GrpcEndpoint, runtime: Runtime },
}

impl HttpEndpoint {
    fn parse(input: &str) -> io::Result<Self> {
        let value = input.strip_prefix("http://").unwrap_or(input).trim();
        if value.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "endpoint must not be empty"));
        }

        let (authority, path) = match value.find('/') {
            Some(index) => (&value[..index], &value[index..]),
            None => (value, "/v1/logs"),
        };
        if authority.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "endpoint authority must not be empty"));
        }

        Ok(Self { authority: authority.to_string(), path: normalize_path(path) })
    }
}

impl LjLogger {
    fn new_http(endpoint: &str, service_name: &str, timeout_ms: u64) -> io::Result<Self> {
        let timeout = Duration::from_millis(timeout_ms.max(1));
        Ok(Self { transport: Transport::Http(HttpEndpoint::parse(endpoint)?), service_name: service_name.to_string(), timeout })
    }

    fn new_grpc(endpoint: &str, service_name: &str, timeout_ms: u64) -> io::Result<Self> {
        let timeout = Duration::from_millis(timeout_ms.max(1));
        let runtime = Runtime::new().map_err(io::Error::other)?;
        Ok(Self { transport: Transport::Grpc { endpoint: GrpcEndpoint::parse(endpoint)?, runtime }, service_name: service_name.to_string(), timeout })
    }

    fn log(&self, record: LogRecordInput) -> io::Result<()> {
        let batch = build_logs_request(&self.service_name, record, self.transport_name());
        match &self.transport {
            Transport::Http(endpoint) => post_otlp_http(endpoint, self.timeout, &batch.encode_to_vec()),
            Transport::Grpc { endpoint, runtime } => runtime.block_on(post_otlp_grpc(endpoint, self.timeout, batch)),
        }
    }

    fn transport_name(&self) -> &'static str {
        match self.transport {
            Transport::Http(_) => "otlp-http",
            Transport::Grpc { .. } => "otlp-grpc",
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lj_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast()
}

#[unsafe(no_mangle)]
pub extern "C" fn lj_error_message() -> *const c_char {
    LAST_ERROR.with(|cell| cell.borrow().as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn lj_logger_new_http(endpoint: *const c_char, service_name: *const c_char, timeout_ms: u64) -> *mut LjLogger {
    ffi_new(|| {
        let endpoint = required_cstr(endpoint, "endpoint")?;
        let service_name = required_cstr(service_name, "service_name")?;
        let logger = LjLogger::new_http(&endpoint, &service_name, timeout_ms)?;
        Ok(Box::into_raw(Box::new(logger)))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lj_logger_new_grpc(endpoint: *const c_char, service_name: *const c_char, timeout_ms: u64) -> *mut LjLogger {
    ffi_new(|| {
        let endpoint = required_cstr(endpoint, "endpoint")?;
        let service_name = required_cstr(service_name, "service_name")?;
        let logger = LjLogger::new_grpc(&endpoint, &service_name, timeout_ms)?;
        Ok(Box::into_raw(Box::new(logger)))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lj_logger_free(logger: *mut LjLogger) {
    if logger.is_null() {
        return;
    }
    let _ = std::panic::catch_unwind(|| {
        // SAFETY: ownership comes from Box::into_raw in lj_logger_new_http.
        unsafe {
            drop(Box::from_raw(logger));
        }
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn lj_logger_log(logger: *mut LjLogger, record: *const lj_log_record) -> bool {
    ffi_bool(|| {
        if logger.is_null() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "logger must not be null"));
        }
        if record.is_null() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "record must not be null"));
        }

        // SAFETY: caller guarantees a valid pointer for the duration of this call.
        let logger = unsafe { &*logger };
        // SAFETY: caller guarantees a valid pointer for the duration of this call.
        let record = unsafe { &*record };
        logger.log(parse_record(record)?)?;
        Ok(())
    })
}

fn build_logs_request(service_name: &str, record: LogRecordInput, transport_name: &str) -> ExportLogsServiceRequest {
    let severity_text = record.severity_text.unwrap_or_else(|| default_severity_text(record.severity_number).to_string());

    let mut attributes = vec![string_attr("liblogjet.transport", transport_name), string_attr("liblogjet.runtime", "cpp-ffi")];
    attributes.extend(record.attributes.into_iter().map(|(key, value)| string_attr(&key, &value)));

    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource { attributes: vec![string_attr("service.name", service_name)], dropped_attributes_count: 0 }),
            scope_logs: vec![ScopeLogs {
                scope: Some(InstrumentationScope {
                    name: "liblogjet".to_string(),
                    version: env!("CARGO_PKG_VERSION").to_string(),
                    attributes: Vec::new(),
                    dropped_attributes_count: 0,
                }),
                log_records: vec![LogRecord {
                    time_unix_nano: record.timestamp_unix_ns,
                    observed_time_unix_nano: record.timestamp_unix_ns,
                    severity_number: normalized_severity(record.severity_number),
                    severity_text,
                    body: Some(AnyValue { value: Some(Value::StringValue(record.body)) }),
                    attributes,
                    dropped_attributes_count: 0,
                    flags: 0,
                    trace_id: Vec::new(),
                    span_id: Vec::new(),
                    event_name: "log".to_string(),
                }],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    }
}

fn post_otlp_http(endpoint: &HttpEndpoint, timeout: Duration, body: &[u8]) -> io::Result<()> {
    let mut stream = TcpStream::connect(&endpoint.authority)?;
    stream.set_write_timeout(Some(timeout))?;
    stream.set_read_timeout(Some(timeout))?;

    write!(
        stream,
        "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/x-protobuf\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        endpoint.path,
        endpoint.authority,
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()?;

    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    if !response.starts_with("HTTP/1.1 200") && !response.starts_with("HTTP/1.0 200") {
        return Err(io::Error::other(format!("collector returned non-200 response: {}", response.lines().next().unwrap_or("unknown response"))));
    }
    Ok(())
}

impl GrpcEndpoint {
    fn parse(input: &str) -> io::Result<Self> {
        let value = input.trim();
        if value.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "endpoint must not be empty"));
        }
        let url = if value.starts_with("http://") || value.starts_with("https://") { value.to_string() } else { format!("http://{value}") };
        Ok(Self { url })
    }
}

async fn post_otlp_grpc(endpoint: &GrpcEndpoint, timeout: Duration, batch: ExportLogsServiceRequest) -> io::Result<()> {
    let channel = tonic::transport::Endpoint::from_shared(endpoint.url.clone())
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))?
        .connect_timeout(timeout)
        .timeout(timeout)
        .connect()
        .await
        .map_err(io::Error::other)?;
    let mut client = LogsServiceClient::new(channel);
    client.export(Request::new(batch)).await.map_err(io::Error::other)?;
    Ok(())
}

fn parse_record(record: &lj_log_record) -> io::Result<LogRecordInput> {
    let body = required_cstr(record.body, "record.body")?;
    let severity_text = optional_cstr(record.severity_text, "record.severity_text")?;
    let mut attributes = Vec::with_capacity(record.attributes_len);

    if record.attributes_len > 0 {
        if record.attributes.is_null() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "record.attributes is null while attributes_len is non-zero"));
        }

        // SAFETY: pointer validity is checked above and length is provided by the caller.
        let slice = unsafe { std::slice::from_raw_parts(record.attributes, record.attributes_len) };
        for attr in slice {
            attributes.push((required_cstr(attr.key, "attribute.key")?, required_cstr(attr.value, "attribute.value")?));
        }
    }

    Ok(LogRecordInput { timestamp_unix_ns: record.timestamp_unix_ns, severity_number: record.severity_number, severity_text, body, attributes })
}

fn required_cstr(ptr: *const c_char, field: &str) -> io::Result<String> {
    if ptr.is_null() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{field} must not be null")));
    }
    // SAFETY: pointer is checked above and treated as read-only.
    let text =
        unsafe { CStr::from_ptr(ptr) }.to_str().map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, format!("{field} must be valid UTF-8")))?;
    Ok(text.to_string())
}

fn optional_cstr(ptr: *const c_char, field: &str) -> io::Result<Option<String>> {
    if ptr.is_null() {
        return Ok(None);
    }
    required_cstr(ptr, field).map(Some)
}

fn normalized_severity(value: i32) -> i32 {
    if value == 0 { SeverityNumber::Info as i32 } else { value }
}

fn default_severity_text(value: i32) -> &'static str {
    match normalized_severity(value) {
        x if x == SeverityNumber::Trace as i32 => "TRACE",
        x if x == SeverityNumber::Debug as i32 => "DEBUG",
        x if x == SeverityNumber::Info as i32 => "INFO",
        x if x == SeverityNumber::Warn as i32 => "WARN",
        x if x == SeverityNumber::Error as i32 => "ERROR",
        x if x == SeverityNumber::Fatal as i32 => "FATAL",
        _ => "INFO",
    }
}

fn string_attr(key: &str, value: &str) -> KeyValue {
    KeyValue { key: key.to_string(), value: Some(AnyValue { value: Some(Value::StringValue(value.to_string())) }) }
}

fn normalize_path(path: &str) -> String {
    if path.is_empty() {
        "/v1/logs".to_string()
    } else if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

fn cstring_lossy(message: &str) -> CString {
    let filtered = message.replace('\0', " ");
    CString::new(filtered).unwrap_or_else(|_| CString::new("ffi error").expect("static string"))
}

fn set_last_error(message: impl Into<String>) {
    let message = cstring_lossy(&message.into());
    LAST_ERROR.with(|cell| {
        *cell.borrow_mut() = message;
    });
}

fn ffi_new<T>(func: impl FnOnce() -> io::Result<*mut T> + std::panic::UnwindSafe) -> *mut T {
    match std::panic::catch_unwind(func) {
        Ok(Ok(value)) => {
            set_last_error("ok");
            value
        }
        Ok(Err(err)) => {
            set_last_error(err.to_string());
            ptr::null_mut()
        }
        Err(_) => {
            set_last_error("panic across FFI boundary");
            ptr::null_mut()
        }
    }
}

fn ffi_bool(func: impl FnOnce() -> io::Result<()> + std::panic::UnwindSafe) -> bool {
    match std::panic::catch_unwind(func) {
        Ok(Ok(())) => {
            set_last_error("ok");
            true
        }
        Ok(Err(err)) => {
            set_last_error(err.to_string());
            false
        }
        Err(_) => {
            set_last_error("panic across FFI boundary");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use tokio::sync::oneshot;
    use tonic::transport::Server;
    use tonic::{Response, Status};

    use opentelemetry_proto::tonic::collector::logs::v1::{
        ExportLogsServiceResponse,
        logs_service_server::{LogsService, LogsServiceServer},
    };

    #[test]
    fn endpoint_parse_defaults_path() {
        let endpoint = HttpEndpoint::parse("127.0.0.1:4318").unwrap();
        assert_eq!(endpoint.authority, "127.0.0.1:4318");
        assert_eq!(endpoint.path, "/v1/logs");
    }

    #[test]
    fn grpc_endpoint_parse_defaults_scheme() {
        let endpoint = GrpcEndpoint::parse("127.0.0.1:4317").unwrap();
        assert_eq!(endpoint.url, "http://127.0.0.1:4317");
    }

    #[test]
    fn ffi_logger_posts_log_record() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let server = thread::spawn(move || -> ExportLogsServiceRequest {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_http_request(&mut stream).unwrap();
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            ExportLogsServiceRequest::decode(request.as_slice()).unwrap()
        });

        let endpoint = CString::new(format!("127.0.0.1:{}", addr.port())).unwrap();
        let service = CString::new("cpp-appliance").unwrap();
        let logger = lj_logger_new_http(endpoint.as_ptr(), service.as_ptr(), 1_000);
        assert!(!logger.is_null(), "ffi init failed: {}", unsafe { CStr::from_ptr(lj_error_message()).to_string_lossy() });

        let severity_text = CString::new("INFO").unwrap();
        let body = CString::new("ffi hello").unwrap();
        let attr_key = CString::new("appliance.id").unwrap();
        let attr_value = CString::new("node-7").unwrap();
        let attributes = [lj_attribute { key: attr_key.as_ptr(), value: attr_value.as_ptr() }];
        let record = lj_log_record {
            timestamp_unix_ns: 123,
            severity_number: SeverityNumber::Info as i32,
            severity_text: severity_text.as_ptr(),
            body: body.as_ptr(),
            attributes: attributes.as_ptr(),
            attributes_len: attributes.len(),
        };

        assert!(lj_logger_log(logger, &record));
        lj_logger_free(logger);

        let batch = server.join().unwrap();
        let resource = &batch.resource_logs[0].resource.as_ref().unwrap().attributes;
        assert!(resource.iter().any(|attr| attr.key == "service.name"));
        let log_record = &batch.resource_logs[0].scope_logs[0].log_records[0];
        assert_eq!(log_record.severity_text, "INFO");
        let body = log_record.body.as_ref().and_then(|value| value.value.as_ref());
        assert!(matches!(body, Some(Value::StringValue(text)) if text == "ffi hello"));
        assert!(log_record.attributes.iter().any(|attr| attr.key == "appliance.id"));
    }

    #[test]
    fn ffi_logger_posts_log_record_over_grpc() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let runtime = Runtime::new().unwrap();
        let addr = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let service = TestLogsService { received: Arc::clone(&received) };
        runtime.spawn(async move {
            Server::builder()
                .add_service(LogsServiceServer::new(service))
                .serve_with_shutdown(addr, async {
                    let _ = shutdown_rx.await;
                })
                .await
                .unwrap();
        });

        let endpoint = CString::new(format!("127.0.0.1:{}", addr.port())).unwrap();
        let service_name = CString::new("cpp-appliance").unwrap();
        let logger = lj_logger_new_grpc(endpoint.as_ptr(), service_name.as_ptr(), 1_000);
        assert!(!logger.is_null(), "ffi init failed: {}", unsafe { CStr::from_ptr(lj_error_message()).to_string_lossy() });

        let severity_text = CString::new("INFO").unwrap();
        let body = CString::new("ffi grpc hello").unwrap();
        let attr_key = CString::new("appliance.id").unwrap();
        let attr_value = CString::new("node-9").unwrap();
        let attributes = [lj_attribute { key: attr_key.as_ptr(), value: attr_value.as_ptr() }];
        let record = lj_log_record {
            timestamp_unix_ns: 456,
            severity_number: SeverityNumber::Info as i32,
            severity_text: severity_text.as_ptr(),
            body: body.as_ptr(),
            attributes: attributes.as_ptr(),
            attributes_len: attributes.len(),
        };

        assert!(lj_logger_log(logger, &record));
        lj_logger_free(logger);

        runtime.block_on(async {
            for _ in 0..50 {
                if !received.lock().unwrap().is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });
        let _ = shutdown_tx.send(());

        let batches = received.lock().unwrap();
        assert_eq!(batches.len(), 1);
        let batch = &batches[0];
        let log_record = &batch.resource_logs[0].scope_logs[0].log_records[0];
        assert_eq!(log_record.severity_text, "INFO");
        let body = log_record.body.as_ref().and_then(|value| value.value.as_ref());
        assert!(matches!(body, Some(Value::StringValue(text)) if text == "ffi grpc hello"));
        assert!(log_record.attributes.iter().any(|attr| attr.key == "appliance.id"));
        assert!(log_record.attributes.iter().any(|attr| attr.key == "liblogjet.transport"));
    }

    fn read_http_request(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
        let mut header = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            stream.read_exact(&mut byte)?;
            header.push(byte[0]);
            if header.ends_with(b"\r\n\r\n") {
                break;
            }
        }

        let header_text =
            std::str::from_utf8(&header[..header.len() - 4]).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid header"))?;
        let mut content_length = None;
        for line in header_text.lines().skip(1) {
            if let Some((name, value)) = line.split_once(':')
                && name.eq_ignore_ascii_case("content-length")
            {
                content_length =
                    Some(value.trim().parse::<usize>().map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid content-length"))?);
            }
        }

        let mut body = vec![0u8; content_length.unwrap_or(0)];
        stream.read_exact(&mut body)?;
        Ok(body)
    }

    #[derive(Clone)]
    struct TestLogsService {
        received: Arc<Mutex<Vec<ExportLogsServiceRequest>>>,
    }

    #[tonic::async_trait]
    impl LogsService for TestLogsService {
        async fn export(&self, request: Request<ExportLogsServiceRequest>) -> Result<Response<ExportLogsServiceResponse>, Status> {
            self.received.lock().unwrap().push(request.into_inner());
            Ok(Response::new(ExportLogsServiceResponse { partial_success: None }))
        }
    }
}
