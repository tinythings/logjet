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
fn http_endpoint_rejects_https_scheme() {
    let err = HttpEndpoint::parse("https://127.0.0.1:4318").unwrap_err();
    assert!(err.to_string().contains("https endpoints are not supported"));
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

    assert!(unsafe { lj_logger_log(logger, &record) });
    unsafe { lj_logger_free(logger) };

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

    assert!(unsafe { lj_logger_log(logger, &record) });
    unsafe { lj_logger_free(logger) };

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

    let header_text = std::str::from_utf8(&header[..header.len() - 4]).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid header"))?;
    let mut content_length = None;
    for line in header_text.lines().skip(1) {
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = Some(value.trim().parse::<usize>().map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid content-length"))?);
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
