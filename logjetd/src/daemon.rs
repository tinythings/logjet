use std::fs;
use std::io::{self, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use opentelemetry_proto::tonic::collector::logs::v1::{
    logs_service_server::{LogsService, LogsServiceServer},
    ExportLogsServiceRequest, ExportLogsServiceResponse,
};
use prost::Message;
use tiny_http::{Method, Response, Server, StatusCode};
use tonic::transport::{Certificate, Identity, ServerTlsConfig};
use tonic::{Request, Response as GrpcResponse, Status};
use rustls::{ServerConfig, ServerConnection, StreamOwned};

use crate::config::{Config, IngestLimits, IngestProtocol, IngestTlsConfig};
use crate::protocol::{read_record_with_limit, read_replay_ack, read_replay_request, write_record};
use crate::spool::Spool;
use crate::tls::{load_ingest_server_config, load_server_config};
use crate::{protocol::WireRecord};

#[derive(Debug, Clone)]
pub struct DaemonConfig {
    pub config: Config,
    pub config_path: PathBuf,
}

pub fn serve(config: DaemonConfig) -> io::Result<()> {
    let spool = Arc::new(Mutex::new(Spool::open(config.config.storage.clone())?));
    let next_seq = Arc::new(AtomicU64::new(1));

    let replay_spool = Arc::clone(&spool);
    let replay_addr = config.config.replay_addr.clone();
    let poll_interval_ms = config.config.poll_interval_ms;
    let tls = config.config.tls.clone();

    let replay_thread = thread::Builder::new()
        .name("logjetd-replay".to_string())
        .spawn(move || replay_loop(replay_addr, replay_spool, poll_interval_ms, tls))?;

    eprintln!("logjetd using config {}", config.config_path.display());
    ingest_loop(
        config.config.ingest_addr,
        config.config.ingest_protocol,
        config.config.ingest_tls,
        config.config.ingest_limits,
        spool,
        next_seq,
    )?;
    replay_thread
        .join()
        .map_err(|_| io::Error::other("replay listener thread panicked"))?
}

fn ingest_loop(
    bind_addr: String,
    protocol: IngestProtocol,
    ingest_tls: IngestTlsConfig,
    ingest_limits: IngestLimits,
    spool: Arc<Mutex<Spool>>,
    next_seq: Arc<AtomicU64>,
) -> io::Result<()> {
    let limiter = Arc::new(IngestLimiter::new(ingest_limits.max_clients));
    match protocol {
        IngestProtocol::Wire => {
            let listener = TcpListener::bind(&bind_addr)?;
            eprintln!(
                "logjetd ingest listening on {bind_addr} using wire protocol max-batch-bytes={} max-clients={}",
                ingest_limits.max_batch_bytes,
                ingest_limits.max_clients
            );

            for stream in listener.incoming() {
                let stream = stream?;
                let spool = Arc::clone(&spool);
                let limiter = Arc::clone(&limiter);
                let max_batch_bytes = ingest_limits.max_batch_bytes;
                thread::Builder::new()
                    .name("logjetd-ingest-client".to_string())
                    .spawn(move || {
                        if let Err(err) = handle_ingest_client(stream, spool, limiter, max_batch_bytes) {
                            eprintln!("logjetd ingest client error: {err}");
                        }
                    })?;
            }
        }
        IngestProtocol::OtlpHttp => {
            if ingest_tls.enable {
                return otlp_http_tls_loop(bind_addr, ingest_tls, ingest_limits, spool, next_seq, limiter);
            }
            let server = Server::http(&bind_addr)
                .map_err(|err| io::Error::other(err.to_string()))?;
            eprintln!(
                "logjetd ingest listening on http://{bind_addr}/v1/logs using otlp-http max-batch-bytes={}",
                ingest_limits.max_batch_bytes
            );

            for mut request in server.incoming_requests() {
                if request.method() != &Method::Post || request.url() != "/v1/logs" {
                    let response = Response::from_string("not found")
                        .with_status_code(StatusCode(404));
                    let _ = request.respond(response);
                    continue;
                }

                let mut body = Vec::with_capacity(ingest_limits.max_batch_bytes.min(8192));
                request
                    .as_reader()
                    .take((ingest_limits.max_batch_bytes + 1) as u64)
                    .read_to_end(&mut body)?;
                if body.len() > ingest_limits.max_batch_bytes {
                    let response = Response::from_string("payload too large")
                        .with_status_code(StatusCode(413));
                    request
                        .respond(response)
                        .map_err(|err| io::Error::other(err.to_string()))?;
                    continue;
                }
                match ExportLogsServiceRequest::decode(body.as_slice()) {
                    Ok(batch) => {
                        let record = WireRecord {
                            record_type: logjet::RecordType::Logs,
                            seq: next_seq.fetch_add(1, Ordering::Relaxed),
                            ts_unix_ns: extract_batch_timestamp(&batch).unwrap_or_else(unix_time_nanos),
                            payload: body,
                        };
                        append_batch_record(&spool, record)?;

                        let response = Response::empty(200);
                        request
                            .respond(response)
                            .map_err(|err| io::Error::other(err.to_string()))?;
                    }
                    Err(err) => {
                        let response = Response::from_string(format!("decode error: {err}"))
                            .with_status_code(StatusCode(400));
                        request
                            .respond(response)
                            .map_err(|resp_err| io::Error::other(resp_err.to_string()))?;
                    }
                }
            }
        }
        IngestProtocol::OtlpGrpc => {
            let addr: SocketAddr = bind_addr.parse().map_err(|err| {
                io::Error::new(io::ErrorKind::InvalidInput, format!("invalid gRPC bind addr: {err}"))
            })?;
            eprintln!(
                "logjetd ingest listening on {}://{bind_addr} using otlp-grpc max-batch-bytes={} max-clients={}",
                if ingest_tls.enable { "grpcs" } else { "grpc" }
                ,
                ingest_limits.max_batch_bytes,
                ingest_limits.max_clients
            );

            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|err| io::Error::other(err.to_string()))?;
            let service = OtlpGrpcLogsService { spool, next_seq };
            let grpc_tls = if ingest_tls.enable {
                Some(build_grpc_server_tls_config(&ingest_tls)?)
            } else {
                None
            };

            runtime.block_on(async move {
                let builder = tonic::transport::Server::builder();
                let builder = if let Some(tls) = grpc_tls {
                    builder
                        .tls_config(tls)
                        .map_err(|err| io::Error::other(err.to_string()))?
                } else {
                    builder
                };

                builder
                    .concurrency_limit_per_connection(ingest_limits.max_clients)
                    .add_service(
                        LogsServiceServer::new(service)
                            .max_decoding_message_size(ingest_limits.max_batch_bytes),
                    )
                    .serve(addr)
                    .await
                    .map_err(|err| io::Error::other(err.to_string()))
            })?;
        }
    }

    Ok(())
}

fn otlp_http_tls_loop(
    bind_addr: String,
    ingest_tls: IngestTlsConfig,
    ingest_limits: IngestLimits,
    spool: Arc<Mutex<Spool>>,
    next_seq: Arc<AtomicU64>,
    limiter: Arc<IngestLimiter>,
) -> io::Result<()> {
    let listener = TcpListener::bind(&bind_addr)?;
    let tls_server = load_ingest_server_config(&ingest_tls)?;
    eprintln!(
        "logjetd ingest listening on https://{bind_addr}/v1/logs using otlp-http max-batch-bytes={} max-clients={}",
        ingest_limits.max_batch_bytes,
        ingest_limits.max_clients
    );

    for stream in listener.incoming() {
        let stream = stream?;
        let spool = Arc::clone(&spool);
        let next_seq = Arc::clone(&next_seq);
        let tls_server = tls_server.clone();
        let limiter = Arc::clone(&limiter);
        let max_batch_bytes = ingest_limits.max_batch_bytes;
        thread::Builder::new()
            .name("logjetd-otlp-http-tls-client".to_string())
            .spawn(move || {
                if let Err(err) = handle_otlp_http_tls_client(
                    stream,
                    tls_server,
                    spool,
                    next_seq,
                    limiter,
                    max_batch_bytes,
                ) {
                    eprintln!("logjetd otlp-http tls client error: {err}");
                }
            })?;
    }

    Ok(())
}

fn handle_otlp_http_tls_client(
    stream: TcpStream,
    tls_server: Arc<ServerConfig>,
    spool: Arc<Mutex<Spool>>,
    next_seq: Arc<AtomicU64>,
    limiter: Arc<IngestLimiter>,
    max_batch_bytes: usize,
) -> io::Result<()> {
    let Some(_permit) = limiter.try_acquire() else {
        eprintln!("logjetd ingest refused TLS client: ingest.max-clients reached");
        return Ok(());
    };
    let conn = ServerConnection::new(tls_server)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))?;
    let mut transport = StreamOwned::new(conn, stream);
    let result = handle_otlp_http_transport(&mut transport, spool, next_seq, max_batch_bytes);
    transport.conn.send_close_notify();
    let _ = transport.flush();
    result
}

fn handle_otlp_http_transport<T: Read + io::Write>(
    transport: &mut T,
    spool: Arc<Mutex<Spool>>,
    next_seq: Arc<AtomicU64>,
    max_batch_bytes: usize,
) -> io::Result<()> {
    let request = match read_http_request(transport, max_batch_bytes) {
        Ok(request) => request,
        Err(err) if err.kind() == io::ErrorKind::InvalidData && err.to_string() == "payload too large" => {
            write_http_response(transport, 413, "payload too large")?;
            return Ok(());
        }
        Err(err) => return Err(err),
    };
    if request.method != "POST" || request.path != "/v1/logs" {
        write_http_response(transport, 404, "not found")?;
        return Ok(());
    }

    match ExportLogsServiceRequest::decode(request.body.as_slice()) {
        Ok(batch) => {
            let record = WireRecord {
                record_type: logjet::RecordType::Logs,
                seq: next_seq.fetch_add(1, Ordering::Relaxed),
                ts_unix_ns: extract_batch_timestamp(&batch).unwrap_or_else(unix_time_nanos),
                payload: request.body,
            };
            append_batch_record(&spool, record)?;
            write_http_response(transport, 200, "")?;
        }
        Err(err) => {
            write_http_response(transport, 400, &format!("decode error: {err}"))?;
        }
    }

    Ok(())
}

fn append_batch_record(spool: &Arc<Mutex<Spool>>, record: WireRecord) -> io::Result<()> {
    let mut spool = spool
        .lock()
        .map_err(|_| io::Error::other("spool mutex poisoned"))?;
    spool.append(record)
}

struct ParsedHttpRequest {
    method: String,
    path: String,
    body: Vec<u8>,
}

fn read_http_request<T: Read>(transport: &mut T, max_batch_bytes: usize) -> io::Result<ParsedHttpRequest> {
    const MAX_HEADER_BYTES: usize = 16 * 1024;
    let mut buffer = Vec::new();
    let mut byte = [0u8; 1];

    loop {
        transport.read_exact(&mut byte)?;
        buffer.push(byte[0]);
        if buffer.len() > MAX_HEADER_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "http header too large"));
        }
        if buffer.ends_with(b"\r\n\r\n") {
            break;
        }
    }

    let header_end = buffer.len();
    let header_text = std::str::from_utf8(&buffer[..header_end - 4])
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "http header is not valid utf-8"))?;
    let mut lines = header_text.lines();
    let request_line = lines
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing http request line"))?;
    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing http method"))?
        .to_string();
    let path = parts
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing http path"))?
        .to_string();

    let mut content_length = None;
    for line in lines {
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = Some(value.trim().parse::<usize>().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid content-length")
            })?);
        }
    }

    let content_length = content_length
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing content-length"))?;
    if content_length > max_batch_bytes {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "payload too large"));
    }
    let mut body = Vec::with_capacity(content_length);
    transport
        .take(content_length as u64)
        .read_to_end(&mut body)?;
    if body.len() != content_length {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "short http body"));
    }

    Ok(ParsedHttpRequest { method, path, body })
}

fn write_http_response<T: io::Write>(transport: &mut T, status: u16, body: &str) -> io::Result<()> {
    let status_text = match status {
        200 => "OK",
        400 => "Bad Request",
        413 => "Payload Too Large",
        404 => "Not Found",
        503 => "Service Unavailable",
        _ => "Error",
    };
    write!(
        transport,
        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status,
        status_text,
        body.len(),
        body
    )?;
    transport.flush()
}

fn build_grpc_server_tls_config(ingest_tls: &IngestTlsConfig) -> io::Result<ServerTlsConfig> {
    let cert_file = ingest_tls.cert_file.as_deref().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "ingest.cert-file is required when ingest.tls-enable is true",
        )
    })?;
    let key_file = ingest_tls.key_file.as_deref().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "ingest.key-file is required when ingest.tls-enable is true",
        )
    })?;

    let identity = Identity::from_pem(fs::read(cert_file)?, fs::read(key_file)?);
    let mut tls = ServerTlsConfig::new().identity(identity);
    if ingest_tls.require_client_cert {
        let ca_file = ingest_tls.ca_file.as_deref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "ingest.ca-file is required when ingest.require-client-cert is true",
            )
        })?;
        tls = tls.client_ca_root(Certificate::from_pem(fs::read(ca_file)?));
    }
    Ok(tls)
}

#[cfg(test)]
#[path = "daemon_utst.rs"]
mod daemon_utst;

fn replay_loop(
    bind_addr: String,
    spool: Arc<Mutex<Spool>>,
    poll_interval_ms: u64,
    tls: crate::config::TlsConfig,
) -> io::Result<()> {
    let listener = TcpListener::bind(&bind_addr)?;
    let tls_server = if tls.enable {
        eprintln!("logjetd replay TLS enabled on {bind_addr}");
        Some(load_server_config(&tls)?)
    } else {
        None
    };
    eprintln!("logjetd replay listening on {bind_addr}");

    for stream in listener.incoming() {
        let stream = stream?;
        let spool = Arc::clone(&spool);
        let tls_server = tls_server.clone();
        thread::Builder::new()
            .name("logjetd-replay-client".to_string())
            .spawn(move || {
                if let Err(err) = handle_replay_client(stream, spool, poll_interval_ms, tls_server) {
                    eprintln!("logjetd replay client error: {err}");
                }
            })?;
    }

    Ok(())
}

fn handle_ingest_client(
    stream: TcpStream,
    spool: Arc<Mutex<Spool>>,
    limiter: Arc<IngestLimiter>,
    max_batch_bytes: usize,
) -> io::Result<()> {
    let Some(_permit) = limiter.try_acquire() else {
        eprintln!("logjetd ingest refused wire client: ingest.max-clients reached");
        return Ok(());
    };
    let peer = stream.peer_addr().ok();
    let mut reader = BufReader::new(stream);

    while let Some(record) = read_record_with_limit(&mut reader, max_batch_bytes)? {
        let mut spool = spool
            .lock()
            .map_err(|_| io::Error::other("spool mutex poisoned"))?;
        spool.append(record)?;
    }

    if let Some(peer) = peer {
        eprintln!("logjetd ingest disconnected: {peer}");
    }
    Ok(())
}

struct IngestLimiter {
    max_clients: usize,
    active_clients: std::sync::atomic::AtomicUsize,
}

impl IngestLimiter {
    fn new(max_clients: usize) -> Self {
        Self {
            max_clients,
            active_clients: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn try_acquire(self: &Arc<Self>) -> Option<IngestPermit> {
        loop {
            let current = self.active_clients.load(Ordering::Relaxed);
            if current >= self.max_clients {
                return None;
            }
            if self
                .active_clients
                .compare_exchange(current, current + 1, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                return Some(IngestPermit {
                    limiter: Arc::clone(self),
                });
            }
        }
    }
}

struct IngestPermit {
    limiter: Arc<IngestLimiter>,
}

impl Drop for IngestPermit {
    fn drop(&mut self) {
        self.limiter.active_clients.fetch_sub(1, Ordering::AcqRel);
    }
}

fn handle_replay_client(
    stream: TcpStream,
    spool: Arc<Mutex<Spool>>,
    poll_interval_ms: u64,
    tls_server: Option<Arc<ServerConfig>>,
) -> io::Result<()> {
    if let Some(server_config) = tls_server {
        let conn = ServerConnection::new(server_config)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))?;
        let mut transport = StreamOwned::new(conn, stream);
        return handle_replay_transport(&mut transport, spool, poll_interval_ms);
    }

    let mut transport = stream;
    handle_replay_transport(&mut transport, spool, poll_interval_ms)
}

fn handle_replay_transport<T: io::Read + io::Write>(
    transport: &mut T,
    spool: Arc<Mutex<Spool>>,
    poll_interval_ms: u64,
) -> io::Result<()> {
    let request = read_replay_request(transport)?;
    let mut last_seq = request.from_seq;
    let sleep = Duration::from_millis(poll_interval_ms);

    eprintln!(
        "logjetd replay client requested records after seq={} mode={}",
        request.from_seq,
        if request.consume { "drain" } else { "keep" }
    );

    if request.consume {
        return handle_replay_transport_drain(transport, spool, &mut last_seq, sleep);
    }

    loop {
        let sent_any = {
            let spool = spool
                .lock()
                .map_err(|_| io::Error::other("spool mutex poisoned"))?;
            spool.replay_since(transport, &mut last_seq)?
        };

        transport.flush()?;
        if !sent_any {
            thread::sleep(sleep);
        }
    }
}

fn handle_replay_transport_drain<T: io::Read + io::Write>(
    transport: &mut T,
    spool: Arc<Mutex<Spool>>,
    last_seq: &mut u64,
    sleep: Duration,
) -> io::Result<()> {
    loop {
        let next_record = {
            let spool = spool
                .lock()
                .map_err(|_| io::Error::other("spool mutex poisoned"))?;
            spool.next_after(*last_seq)?
        };

        let Some(record) = next_record else {
            thread::sleep(sleep);
            continue;
        };

        write_record(transport, &record)?;
        transport.flush()?;

        let ack = read_replay_ack(transport)?;
        if ack.ack_seq != record.seq {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("replay ack out of order: expected {} got {}", record.seq, ack.ack_seq),
            ));
        }

        {
            let mut spool = spool
                .lock()
                .map_err(|_| io::Error::other("spool mutex poisoned"))?;
            spool.consume_through(ack.ack_seq)?;
        }
        *last_seq = ack.ack_seq;
    }
}

fn extract_batch_timestamp(batch: &ExportLogsServiceRequest) -> Option<u64> {
    for resource_logs in &batch.resource_logs {
        for scope_logs in &resource_logs.scope_logs {
            for record in &scope_logs.log_records {
                if record.time_unix_nano != 0 {
                    return Some(record.time_unix_nano);
                }
                if record.observed_time_unix_nano != 0 {
                    return Some(record.observed_time_unix_nano);
                }
            }
        }
    }
    None
}

fn unix_time_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

#[derive(Clone)]
struct OtlpGrpcLogsService {
    spool: Arc<Mutex<Spool>>,
    next_seq: Arc<AtomicU64>,
}

#[tonic::async_trait]
impl LogsService for OtlpGrpcLogsService {
    async fn export(
        &self,
        request: Request<ExportLogsServiceRequest>,
    ) -> Result<GrpcResponse<ExportLogsServiceResponse>, Status> {
        let batch = request.into_inner();
        let payload = batch.encode_to_vec();
        let record = WireRecord {
            record_type: logjet::RecordType::Logs,
            seq: self.next_seq.fetch_add(1, Ordering::Relaxed),
            ts_unix_ns: extract_batch_timestamp(&batch).unwrap_or_else(unix_time_nanos),
            payload,
        };

        let mut spool = self
            .spool
            .lock()
            .map_err(|_| Status::internal("spool mutex poisoned"))?;
        spool
            .append(record)
            .map_err(|err| Status::internal(err.to_string()))?;

        Ok(GrpcResponse::new(ExportLogsServiceResponse {
            partial_success: None,
        }))
    }
}
