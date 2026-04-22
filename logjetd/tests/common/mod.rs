use std::fs;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use opentelemetry_proto::tonic::collector::logs::v1::{
    ExportLogsServiceRequest, ExportLogsServiceResponse,
    logs_service_server::{LogsService, LogsServiceServer},
};
use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope, KeyValue};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs, SeverityNumber};
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message;
use rcgen::{BasicConstraints, Certificate, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa, SanType};
use tokio::runtime::Builder;
use tonic::transport::{Identity, ServerTlsConfig};
use tonic::{Request, Response, Status};

const REPLAY_HELLO_MAGIC: [u8; 8] = *b"LJRPH001";
const REPLAY_REQUEST_MAGIC: [u8; 8] = *b"LJRPL001";
const WIRE_MAGIC: [u8; 8] = *b"LJNETV01";

pub struct TestDir {
    path: PathBuf,
}

impl TestDir {
    pub fn new(label: &str) -> io::Result<Self> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("ljd-it-{label}-{nanos}-{}", std::process::id()));
        fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn write(&self, name: &str, body: &str) -> io::Result<PathBuf> {
        let path = self.path.join(name);
        fs::write(&path, body)?;
        Ok(path)
    }
}

pub struct GrpcTlsFiles {
    pub ca: PathBuf,
    pub client_cert: PathBuf,
    pub client_key: PathBuf,
    server_cert: PathBuf,
    server_key: PathBuf,
}

pub fn write_fake_grpc_tls_files(dir: &TestDir) -> io::Result<GrpcTlsFiles> {
    let ca = new_ca_cert()?;
    let server = new_signed_cert(
        "collector.test.invalid",
        &[SanType::DnsName("collector.test.invalid".into()), SanType::IpAddress("127.0.0.1".parse().unwrap())],
        ExtendedKeyUsagePurpose::ServerAuth,
        &ca,
    )?;
    let client = new_signed_cert("client.test.invalid", &[SanType::DnsName("client.test.invalid".into())], ExtendedKeyUsagePurpose::ClientAuth, &ca)?;
    Ok(GrpcTlsFiles {
        ca: dir.write("grpc-ca.pem", &ca.serialize_pem().map_err(rcgen_err)?)?,
        client_cert: dir.write("grpc-client.pem", &client.serialize_pem_with_signer(&ca).map_err(rcgen_err)?)?,
        client_key: dir.write("grpc-client.key", &client.serialize_private_key_pem())?,
        server_cert: dir.write("grpc-server.pem", &server.serialize_pem_with_signer(&ca).map_err(rcgen_err)?)?,
        server_key: dir.write("grpc-server.key", &server.serialize_private_key_pem())?,
    })
}

fn new_ca_cert() -> io::Result<Certificate> {
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.distinguished_name = DistinguishedName::new();
    params.distinguished_name.push(DnType::CommonName, "Fake Test CA");
    Certificate::from_params(params).map_err(rcgen_err)
}

fn new_signed_cert(name: &str, sans: &[SanType], usage: ExtendedKeyUsagePurpose, ca: &Certificate) -> io::Result<Certificate> {
    let mut params = CertificateParams::default();
    params.distinguished_name = DistinguishedName::new();
    params.distinguished_name.push(DnType::CommonName, name);
    params.extended_key_usages = vec![usage];
    params.subject_alt_names = sans.to_vec();
    let cert = Certificate::from_params(params).map_err(rcgen_err)?;
    let _ = cert.serialize_pem_with_signer(ca).map_err(rcgen_err)?;
    Ok(cert)
}

fn rcgen_err(err: rcgen::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, err.to_string())
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub struct ChildGuard {
    child: Child,
}

impl ChildGuard {
    pub fn spawn(mut command: Command) -> io::Result<Self> {
        let child = command.stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
        Ok(Self { child })
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn ljd_command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ljd"))
}

pub fn free_port() -> io::Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

pub fn wait_for_tcp(addr: &str, timeout: Duration) -> io::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        match TcpStream::connect(addr) {
            Ok(stream) => {
                let _ = stream.shutdown(Shutdown::Both);
                return Ok(());
            }
            Err(err) if Instant::now() < deadline => {
                if err.kind() != io::ErrorKind::ConnectionRefused
                    && err.kind() != io::ErrorKind::TimedOut
                    && err.kind() != io::ErrorKind::AddrNotAvailable
                {
                    return Err(err);
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(err) => return Err(err),
        }
    }
}

pub fn wait_until<F>(timeout: Duration, mut predicate: F) -> io::Result<()>
where
    F: FnMut() -> io::Result<bool>,
{
    let deadline = Instant::now() + timeout;
    loop {
        if predicate()? {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "timed out waiting for condition"));
        }
        thread::sleep(Duration::from_millis(25));
    }
}

pub struct MockCollector {
    received: Arc<Mutex<Vec<ExportLogsServiceRequest>>>,
    _thread: thread::JoinHandle<()>,
}

impl MockCollector {
    pub fn start(port: u16) -> io::Result<Self> {
        Self::start_with_delay(port, Duration::ZERO)
    }

    pub fn start_with_delay(port: u16, delay: Duration) -> io::Result<Self> {
        let addr = format!("127.0.0.1:{port}");
        let listener = TcpListener::bind(&addr)?;
        listener.set_nonblocking(false)?;
        let received = Arc::new(Mutex::new(Vec::new()));
        let received_thread = Arc::clone(&received);

        let thread = thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else {
                    break;
                };
                let _ = handle_http_request(&mut stream, &received_thread, delay);
            }
        });

        let _ = addr;
        Ok(Self { received, _thread: thread })
    }

    pub fn messages(&self) -> Vec<String> {
        self.received.lock().unwrap().iter().flat_map(extract_messages).collect()
    }
}

pub struct MockGrpcCollector {
    received: Arc<Mutex<Vec<ExportLogsServiceRequest>>>,
    _thread: thread::JoinHandle<()>,
}

impl MockGrpcCollector {
    pub fn start(port: u16) -> io::Result<Self> {
        let received = Arc::new(Mutex::new(Vec::new()));
        let service = TestGrpcCollector { received: Arc::clone(&received) };
        let addr = format!("127.0.0.1:{port}")
            .parse()
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, format!("invalid gRPC collector address: {err}")))?;
        let thread = thread::spawn(move || {
            let runtime = Builder::new_current_thread().enable_all().build().expect("gRPC test runtime");
            runtime.block_on(async move {
                tonic::transport::Server::builder().add_service(LogsServiceServer::new(service)).serve(addr).await.expect("gRPC collector server");
            });
        });
        Ok(Self { received, _thread: thread })
    }

    pub fn messages(&self) -> Vec<String> {
        self.received.lock().unwrap().iter().flat_map(extract_messages).collect()
    }

    pub fn start_tls(port: u16, tls: &GrpcTlsFiles) -> io::Result<Self> {
        let received = Arc::new(Mutex::new(Vec::new()));
        let service = TestGrpcCollector { received: Arc::clone(&received) };
        let server_cert = fs::read(&tls.server_cert)?;
        let server_key = fs::read(&tls.server_key)?;
        let addr = format!("127.0.0.1:{port}")
            .parse()
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, format!("invalid gRPC collector address: {err}")))?;
        let thread = thread::spawn(move || {
            let runtime = Builder::new_current_thread().enable_all().build().expect("gRPC TLS test runtime");
            runtime.block_on(async move {
                tonic::transport::Server::builder()
                    .tls_config(ServerTlsConfig::new().identity(Identity::from_pem(server_cert, server_key)))
                    .expect("gRPC TLS config")
                    .add_service(LogsServiceServer::new(service))
                    .serve(addr)
                    .await
                    .expect("gRPC TLS collector server");
            });
        });
        Ok(Self { received, _thread: thread })
    }

    pub fn start_mtls(port: u16, tls: &GrpcTlsFiles) -> io::Result<Self> {
        let received = Arc::new(Mutex::new(Vec::new()));
        let service = TestGrpcCollector { received: Arc::clone(&received) };
        let ca = fs::read(&tls.ca)?;
        let server_cert = fs::read(&tls.server_cert)?;
        let server_key = fs::read(&tls.server_key)?;
        let addr = format!("127.0.0.1:{port}")
            .parse()
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, format!("invalid gRPC collector address: {err}")))?;
        let thread = thread::spawn(move || {
            let runtime = Builder::new_current_thread().enable_all().build().expect("gRPC mTLS test runtime");
            runtime.block_on(async move {
                tonic::transport::Server::builder()
                    .tls_config(
                        ServerTlsConfig::new()
                            .identity(Identity::from_pem(server_cert, server_key))
                            .client_ca_root(tonic::transport::Certificate::from_pem(ca)),
                    )
                    .expect("gRPC mTLS config")
                    .add_service(LogsServiceServer::new(service))
                    .serve(addr)
                    .await
                    .expect("gRPC mTLS collector server");
            });
        });
        Ok(Self { received, _thread: thread })
    }
}

#[derive(Clone)]
struct TestGrpcCollector {
    received: Arc<Mutex<Vec<ExportLogsServiceRequest>>>,
}

#[tonic::async_trait]
impl LogsService for TestGrpcCollector {
    async fn export(&self, request: Request<ExportLogsServiceRequest>) -> Result<Response<ExportLogsServiceResponse>, Status> {
        self.received.lock().unwrap().push(request.into_inner());
        Ok(Response::new(ExportLogsServiceResponse { partial_success: None }))
    }
}

fn handle_http_request(stream: &mut TcpStream, received: &Arc<Mutex<Vec<ExportLogsServiceRequest>>>, delay: Duration) -> io::Result<()> {
    let request = read_http_request(stream)?;
    if request.method != "POST" || request.path != "/v1/logs" {
        write_http_response(stream, 404, "not found")?;
        return Ok(());
    }
    let batch =
        ExportLogsServiceRequest::decode(request.body.as_slice()).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;
    received.lock().unwrap().push(batch);
    if !delay.is_zero() {
        thread::sleep(delay);
    }
    write_http_response(stream, 200, "")
}

struct HttpRequest {
    method: String,
    path: String,
    body: Vec<u8>,
}

fn read_http_request(stream: &mut TcpStream) -> io::Result<HttpRequest> {
    let mut header = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        stream.read_exact(&mut byte)?;
        header.push(byte[0]);
        if header.ends_with(b"\r\n\r\n") {
            break;
        }
        if header.len() > 16 * 1024 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "header too large"));
        }
    }

    let header_text = std::str::from_utf8(&header[..header.len() - 4]).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid header"))?;
    let mut lines = header_text.lines();
    let request_line = lines.next().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing request line"))?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing method"))?.to_string();
    let path = parts.next().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing path"))?.to_string();
    let mut content_length = None;
    for line in lines {
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = Some(value.trim().parse::<usize>().map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid content-length"))?);
        }
    }
    let content_length = content_length.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing content-length"))?;
    let mut body = vec![0u8; content_length];
    stream.read_exact(&mut body)?;
    Ok(HttpRequest { method, path, body })
}

fn write_http_response(stream: &mut TcpStream, status: u16, body: &str) -> io::Result<()> {
    let status_text = match status {
        200 => "OK",
        404 => "Not Found",
        _ => "Error",
    };
    write!(stream, "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", status, status_text, body.len(), body)?;
    stream.flush()
}

pub fn post_otlp_http(addr: &str, service_name: &str, message: &str) -> io::Result<()> {
    let batch = build_logs_request(service_name, message);
    let body = batch.encode_to_vec();
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut stream = loop {
        match TcpStream::connect(addr) {
            Ok(stream) => break stream,
            Err(err) if err.kind() == io::ErrorKind::ConnectionRefused && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(25));
            }
            Err(err) => return Err(err),
        }
    };
    write!(
        stream,
        "POST /v1/logs HTTP/1.1\r\nHost: {}\r\nContent-Type: application/x-protobuf\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        addr,
        body.len()
    )?;
    stream.write_all(&body)?;
    stream.flush()?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    if !response.starts_with("HTTP/1.1 200") {
        return Err(io::Error::other(format!("non-200 response: {}", response.lines().next().unwrap_or("unknown"))));
    }
    Ok(())
}

pub fn replay_messages(addr: &str, from_seq: u64, limit: usize) -> io::Result<Vec<String>> {
    let mut stream = connect_replay_client(addr, from_seq, false)?;

    let mut messages = Vec::new();
    for _ in 0..limit {
        match read_replay_payload(&mut stream)? {
            Some(record) => {
                let batch =
                    ExportLogsServiceRequest::decode(record.as_slice()).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;
                messages.extend(extract_messages(&batch));
            }
            None => break,
        }
    }
    Ok(messages)
}

pub fn connect_replay_client(addr: &str, from_seq: u64, consume: bool) -> io::Result<TcpStream> {
    let mut stream = TcpStream::connect(addr)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    read_replay_hello(&mut stream)?;
    write_replay_request(&mut stream, from_seq, consume)?;
    stream.flush()?;
    Ok(stream)
}

pub fn read_replay_payload(stream: &mut TcpStream) -> io::Result<Option<Vec<u8>>> {
    read_wire_record(stream)
}

pub fn read_replay_message(stream: &mut TcpStream) -> io::Result<Option<String>> {
    let Some(payload) = read_replay_payload(stream)? else {
        return Ok(None);
    };
    let batch = ExportLogsServiceRequest::decode(payload.as_slice()).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;
    Ok(extract_messages(&batch).into_iter().next())
}

fn read_replay_hello(stream: &mut TcpStream) -> io::Result<()> {
    let mut magic = [0u8; 8];
    stream.read_exact(&mut magic)?;
    if magic != REPLAY_HELLO_MAGIC {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid replay hello magic"));
    }
    let mut header = [0u8; 32];
    stream.read_exact(&mut header)?;
    Ok(())
}

fn write_replay_request(stream: &mut TcpStream, from_seq: u64, consume: bool) -> io::Result<()> {
    stream.write_all(&REPLAY_REQUEST_MAGIC)?;
    stream.write_all(&[1u8])?;
    stream.write_all(&[u8::from(consume)])?;
    stream.write_all(&[0u8; 6])?;
    stream.write_all(&from_seq.to_le_bytes())?;
    Ok(())
}

fn read_wire_record(stream: &mut TcpStream) -> io::Result<Option<Vec<u8>>> {
    let mut magic = [0u8; 8];
    match stream.read_exact(&mut magic) {
        Ok(()) => {}
        Err(err)
            if err.kind() == io::ErrorKind::UnexpectedEof || err.kind() == io::ErrorKind::TimedOut || err.kind() == io::ErrorKind::WouldBlock =>
        {
            return Ok(None);
        }
        Err(err) => return Err(err),
    }
    if magic != WIRE_MAGIC {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid wire magic"));
    }
    let mut header = [0u8; 24];
    stream.read_exact(&mut header)?;
    let codec = header[2];
    let payload_len = u32::from_le_bytes([header[20], header[21], header[22], header[23]]) as usize;
    let mut payload = vec![0u8; payload_len];
    stream.read_exact(&mut payload)?;
    let mut _crc = [0u8; 4];
    stream.read_exact(&mut _crc)?;
    if codec == 1 {
        if payload.len() < 4 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "LZ4 payload too short"));
        }
        let uncompressed_len = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;
        let decompressed = lz4_flex::block::decompress(&payload[4..], uncompressed_len)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;
        Ok(Some(decompressed))
    } else {
        Ok(Some(payload))
    }
}

fn build_logs_request(service_name: &str, message: &str) -> ExportLogsServiceRequest {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64;
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: vec![KeyValue {
                    key: "service.name".to_string(),
                    value: Some(AnyValue {
                        value: Some(opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(service_name.to_string())),
                    }),
                }],
                dropped_attributes_count: 0,
                entity_refs: Vec::new(),
            }),
            scope_logs: vec![ScopeLogs {
                scope: Some(InstrumentationScope {
                    name: "bridge-flow-test".to_string(),
                    version: String::new(),
                    attributes: Vec::new(),
                    dropped_attributes_count: 0,
                }),
                log_records: vec![LogRecord {
                    time_unix_nano: nanos,
                    observed_time_unix_nano: nanos,
                    severity_number: SeverityNumber::Info as i32,
                    severity_text: "INFO".to_string(),
                    body: Some(AnyValue { value: Some(opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(message.to_string())) }),
                    ..Default::default()
                }],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    }
}

fn extract_messages(batch: &ExportLogsServiceRequest) -> Vec<String> {
    let mut messages = Vec::new();
    for resource_logs in &batch.resource_logs {
        for scope_logs in &resource_logs.scope_logs {
            for record in &scope_logs.log_records {
                if let Some(body) = &record.body
                    && let Some(opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(value)) = &body.value
                {
                    messages.push(value.clone());
                }
            }
        }
    }
    messages
}
