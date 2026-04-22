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

    pub fn start_tls(port: u16) -> io::Result<Self> {
        let received = Arc::new(Mutex::new(Vec::new()));
        let service = TestGrpcCollector { received: Arc::clone(&received) };
        let addr = format!("127.0.0.1:{port}")
            .parse()
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, format!("invalid gRPC collector address: {err}")))?;
        let thread = thread::spawn(move || {
            let runtime = Builder::new_current_thread().enable_all().build().expect("gRPC TLS test runtime");
            runtime.block_on(async move {
                tonic::transport::Server::builder()
                    .tls_config(
                        ServerTlsConfig::new().identity(Identity::from_pem(STARWARS_GRPC_SERVER_PEM.as_bytes(), STARWARS_GRPC_SERVER_KEY.as_bytes())),
                    )
                    .expect("gRPC TLS config")
                    .add_service(LogsServiceServer::new(service))
                    .serve(addr)
                    .await
                    .expect("gRPC TLS collector server");
            });
        });
        Ok(Self { received, _thread: thread })
    }
}

#[derive(Clone)]
struct TestGrpcCollector {
    received: Arc<Mutex<Vec<ExportLogsServiceRequest>>>,
}

pub const STARWARS_GRPC_CA_PEM: &str = r#"-----BEGIN CERTIFICATE-----
MIIDHzCCAgegAwIBAgIUe4n6dgXeznHVbLSs8bKMM5GKU8wwDQYJKoZIhvcNAQEL
BQAwFzEVMBMGA1UEAwwMSmVkaSBUZXN0IENBMB4XDTI2MDQyMjE3MjcyOVoXDTM2
MDQxOTE3MjcyOVowFzEVMBMGA1UEAwwMSmVkaSBUZXN0IENBMIIBIjANBgkqhkiG
9w0BAQEFAAOCAQ8AMIIBCgKCAQEA0jLB8frzlXteabmTSsVo/XpRcY3ix0zrnJQg
zaONmULZVbblZKqq1wqjUzMZsVCQ8S4E/i1b5sIibUxCHuyWbbHA7E40yLEakI9E
8f/Qd9638W8bLSmLxkzt9EZE4t3yPoh3NATYy7XzzyfDhSBhSBbAj49OY0jAleJw
GsO8SuO+HukNcjE+gwxvZU+g/j+j09RVptFVApOBDucrnVXuj61FV+hvCHzVIyOA
cqq4IDMrx1LtxHes9PgEhMFYp6T2KajqJw5q8PyJ0rqpWwv5+BorCMPvnST5D7QF
zrSvUvI1V+5Jt9SE0Fr+nApK98tdGMKTv9hjQ3WCoD8yIca3HQIDAQABo2MwYTAP
BgNVHRMBAf8EBTADAQH/MA4GA1UdDwEB/wQEAwIBBjAdBgNVHQ4EFgQUZvG+a0HI
cOD5b1czNqMoHKZ0j5UwHwYDVR0jBBgwFoAUZvG+a0HIcOD5b1czNqMoHKZ0j5Uw
DQYJKoZIhvcNAQELBQADggEBAITLql7okWYkl060FiNq34Jw/NlKIdgr7L/nJ20M
avDrax4dEP4CwBEGSDtAYq2ae4nkSgKtFlDA1wJAqzGKbl4ETB4NHdAJ8HfN52X5
b3GphAKSznpbPrWIYRtJyIc9NNPXqqvtj68aSxHcW1vnKuv1tCBdbeenBMbzMnna
1RsgzYqnxvZhOyh2JNuv3RHxGBl9kHqtqprJyooKTXD7fqM0ta1OIeaq4P7VXgfE
xM+EWIqjcOi5FceBdsLdYHSkc0kppLILacsmiAL6sWhDgBIYUVF4sV3GAbooYQYg
QllrT/aXmuFAcBXvVwk/02OvUH5RpKKH43iPRfSS9Rfs9qw=
-----END CERTIFICATE-----
"#;

const STARWARS_GRPC_SERVER_PEM: &str = r#"-----BEGIN CERTIFICATE-----
MIIDSTCCAjGgAwIBAgIUUjZ85YsHnc9UdpTbQ0HxcOqmi9swDQYJKoZIhvcNAQEL
BQAwFzEVMBMGA1UEAwwMSmVkaSBUZXN0IENBMB4XDTI2MDQyMjE3MjcyOVoXDTM2
MDQxOTE3MjcyOVowIjEgMB4GA1UEAwwXY29sbGVjdG9yLnN0YXJ3YXJzLnRlc3Qw
ggEiMA0GCSqGSIb3DQEBAQUAA4IBDwAwggEKAoIBAQCzURjh/BXlLu+JDaWiBcGG
q5St/NOZ2wexydD3bHBfdk/LcGTQhxPVWlTrSofoIdbfeeIVZtdbv6oDUIxrKMxc
FYJjZpZRhjGaxMYOZXpSNkRjrTm2lvh3A+2Bc1d4HyNSVZMG9tdQ5Pw+V4O32MxV
anf05oBaD6f7OS0bcSTSFW7dSuxMmhAu1UXBN+Q6GlMlx1WgnEE1WkoDjt+Xy+YD
cnEAvlVp3HdOIcrINC6CJ8KNeauWoIM/AdAPtphVk/YyQXhCVK/cNMBQU/P62Sma
7ch5OuZQwpdESDB5IMWAAWiulEFRZaiJDSvMAApWzeh4HMl8jAg6DWk2WX0nQOAh
AgMBAAGjgYEwfzAoBgNVHREEITAfghdjb2xsZWN0b3Iuc3RhcndhcnMudGVzdIcE
fwAAATATBgNVHSUEDDAKBggrBgEFBQcDATAdBgNVHQ4EFgQUhHVk0qqjYf6d06Ey
+0OPBAuQSsUwHwYDVR0jBBgwFoAUZvG+a0HIcOD5b1czNqMoHKZ0j5UwDQYJKoZI
hvcNAQELBQADggEBAEleld5ECRhFXfERznuttsJFI47IIGvUGirq1a1+yCjsLalv
GnzmuV3pEKyvFQ8ZKFRvpz4+hEjK+5bkoPrmKW8c14uQEDUa3OtyWL9rQrxQXGB7
6pjK1SetRstqT7GnUeRf5PECWqeFSWFVUUwyVLZHbrY3u/sKP0okYQgLtoxjj9kF
MCcrkH9heMZhZBFA14asl66t7QILEQitBOtqw6mGcrZ34MBkvF47gC0b1ssk3aJk
+Y7tO922Vg9E2HgG9l4A/+y1pNEwkBJ/AwB5at/BW+HzHLLbHqmJiQYb0YRUhJWZ
L2iP3SyD+8gFLX4ZhqKJnHDs0uMxRC5I8OIjJoo=
-----END CERTIFICATE-----
"#;

const STARWARS_GRPC_SERVER_KEY: &str = r#"-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQCzURjh/BXlLu+J
DaWiBcGGq5St/NOZ2wexydD3bHBfdk/LcGTQhxPVWlTrSofoIdbfeeIVZtdbv6oD
UIxrKMxcFYJjZpZRhjGaxMYOZXpSNkRjrTm2lvh3A+2Bc1d4HyNSVZMG9tdQ5Pw+
V4O32MxVanf05oBaD6f7OS0bcSTSFW7dSuxMmhAu1UXBN+Q6GlMlx1WgnEE1WkoD
jt+Xy+YDcnEAvlVp3HdOIcrINC6CJ8KNeauWoIM/AdAPtphVk/YyQXhCVK/cNMBQ
U/P62Sma7ch5OuZQwpdESDB5IMWAAWiulEFRZaiJDSvMAApWzeh4HMl8jAg6DWk2
WX0nQOAhAgMBAAECggEAAlznnQkhwlSTmq5WKAQEM6GzL3YmohJjnMSLpZcFbA/Q
QV5u/A7e0TyD5Fql3wZph6Xz6rnhZwSBjJ87gNxkXppFKGL24oaIKzS7zDJeIeAB
MRpC3NSxQknU9i27Ub2A5toMOlbdkEog+mYg02orilY0balT66ix9MeHs2+sP1bV
suP88SdAi77dpyEgxq8ZBW8PLXn5U6Dft5pSdhmyHBGItG5t9maq/VcB7oBh29i7
M/giamiZ7DBc8o5iXLp1lipNd0tV6h07dgp25m4d61oPZ0LRRZ+tUvug/kW36R+3
KEZ1OUYye8P/yTzO8+0UOXKbEGcTlXaYl2rSftv0gQKBgQDoR8GVhj859cYp3y8A
6DvvV3CsdqsyjlYsVJis98ZGW4AZZ0pDP4EcD+Hce44uAO1upbVoUICor7okOD4Z
eV1sWr4qaCvJXOGuUQKBxxdFFmpWjEBFaYVUneGyXv7j+kKiF4Qqc2HiP+OAvoM9
YM1AbSg/1xdcIbQi6oXBjUC2OQKBgQDFoMZ0Msq9np0Z3l1XGmnraj0a1eMu51eM
aIR3gnNcQfXoRO4Vtq3yEPSdk+/EM5/fBXzi1NKQdEdz9U/oiPfWnGCr4b98ay7L
Tk9jeJWPqHKAl8cg1O9QU9PxYn0CeSsv4tOllykyb45Z8AYNXaBUh5Ej2TsMsOY9
/frE8dk5KQKBgQDlGBDIZvXpNozSM3vqiyLB9x38G7bSUCyR4IYM4vw93HVFmOhX
10SB5vA/Q+WBXgzPusRnNC8RMPCIVKh6+4a3HfC9Zqz5F5DHGsM8OJ6s12TeI8oo
K+EDCgzWnncLZ4Nc15DVRaPfQGAkVMKgQN9vkbnG7V/u0JcYcPKnaafPkQKBgFBJ
AFY0TCi8RxY7P7AjCuSYRDqiqahkUyy3SRlD5ZmVMlEpr48ip4evW7CoaL9MOaZg
lFuSGfiVRHHXNp9BBW4qGRu6mg/xexEcvyOp2RiDVgDnp/2ug4oeg/uMBzz5/JF3
lIOw5QuYRjxDRjIn1vqAGHZ3yYVeWCrXAwj/N0ABAoGBAOcFm8sy0jsxmGI3wJvm
OiMR4V0CmzsWyE1P2QD4oTSogm4RAZNcV2r17EfoDqEBJjdwi1tT9yAuwSypSGE5
ynaGxpq9ZoqcE76+9Lp+X5Ff9Rrs0c0aKMQgdsZTHh/kFg36PDjBbLwNRmg6yjhw
5P8OAY++sbRc+7FSh03+FRVG
-----END PRIVATE KEY-----
"#;

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
    let mut stream = TcpStream::connect(addr)?;
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
