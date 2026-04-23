use std::fs;
use std::io::{self, BufReader, Read, Write};
use std::net::SocketAddr;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use opentelemetry_proto::tonic::collector::logs::v1::{
    ExportLogsServiceRequest, ExportLogsServiceResponse,
    logs_service_server::{LogsService, LogsServiceServer},
};
use prost::Message;
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use tiny_http::{Method, Response, Server, StatusCode};
use tonic::transport::{Certificate, Identity, ServerTlsConfig};
use tonic::{Request, Response as GrpcResponse, Status};

use crate::config::{Config, IngestLimits, IngestOverloadConfig, IngestProtocol, IngestTlsConfig, SeverityFloor};
use crate::protocol::WireRecord;
use crate::protocol::{ReplayHello, read_record_with_limit, read_replay_ack, read_replay_request, write_record, write_replay_hello};
use crate::spool::Spool;
use crate::tls::{load_ingest_server_config, load_server_config};

#[derive(Debug, Clone)]
pub struct DaemonConfig {
    pub config: Config,
    pub config_path: PathBuf,
}

pub(crate) struct SharedSpool {
    spool: Mutex<Spool>,
    wake_state: Mutex<u64>,
    wake_cv: Condvar,
}

struct SharedIngestPolicy {
    inner: Mutex<IngestPolicyState>,
    config: IngestOverloadConfig,
}

#[derive(Debug)]
struct IngestPolicyState {
    window_start: Instant,
    accepted_in_window: u64,
    stats: IngestOverloadStats,
    next_report_at: Instant,
}

#[derive(Debug, Default, Clone, Copy)]
struct IngestOverloadStats {
    accepted: u64,
    priority_bypass: u64,
    rate_limited: u64,
    oversize_rejected: u64,
    client_cap_rejected: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum BatchPriority {
    Unknown = 0,
    Trace = 1,
    Debug = 2,
    Info = 3,
    Warn = 4,
    Error = 5,
    Fatal = 6,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IngestDecision {
    Accept,
    AcceptPriorityBypass,
    RejectRateLimited,
}

impl SharedSpool {
    fn new(spool: Spool) -> Self {
        Self { spool: Mutex::new(spool), wake_state: Mutex::new(0), wake_cv: Condvar::new() }
    }

    fn notify_change(&self) -> io::Result<()> {
        let mut generation = self.wake_state.lock().map_err(|_| io::Error::other("wake-state mutex poisoned"))?;
        *generation = generation.saturating_add(1);
        self.wake_cv.notify_all();
        Ok(())
    }

    fn wait_for_change(&self, seen_generation: &mut u64) -> io::Result<()> {
        let generation = self.wake_state.lock().map_err(|_| io::Error::other("wake-state mutex poisoned"))?;
        let generation =
            self.wake_cv.wait_while(generation, |current| *current == *seen_generation).map_err(|_| io::Error::other("wake-state mutex poisoned"))?;
        *seen_generation = *generation;
        Ok(())
    }

    fn current_generation(&self) -> io::Result<u64> {
        let generation = self.wake_state.lock().map_err(|_| io::Error::other("wake-state mutex poisoned"))?;
        Ok(*generation)
    }
}

impl SharedIngestPolicy {
    fn new(config: IngestOverloadConfig) -> Self {
        let now = Instant::now();
        Self {
            inner: Mutex::new(IngestPolicyState {
                window_start: now,
                accepted_in_window: 0,
                stats: IngestOverloadStats::default(),
                next_report_at: now + Duration::from_millis(config.report_every_ms.max(1)),
            }),
            config,
        }
    }

    fn decide(&self, priority: BatchPriority) -> io::Result<IngestDecision> {
        let mut state = self.inner.lock().map_err(|_| io::Error::other("ingest policy mutex poisoned"))?;
        let now = Instant::now();
        if now.duration_since(state.window_start) >= Duration::from_secs(1) {
            state.window_start = now;
            state.accepted_in_window = 0;
        }

        let decision = if self.config.max_batches_per_second == 0 || state.accepted_in_window < self.config.max_batches_per_second {
            state.accepted_in_window = state.accepted_in_window.saturating_add(1);
            state.stats.accepted = state.stats.accepted.saturating_add(1);
            IngestDecision::Accept
        } else if priority >= batch_priority_floor(self.config.priority_severity_floor) {
            state.stats.priority_bypass = state.stats.priority_bypass.saturating_add(1);
            IngestDecision::AcceptPriorityBypass
        } else {
            state.stats.rate_limited = state.stats.rate_limited.saturating_add(1);
            IngestDecision::RejectRateLimited
        };

        if matches!(decision, IngestDecision::AcceptPriorityBypass | IngestDecision::RejectRateLimited) {
            maybe_report_overload(&self.config, &mut state);
        }

        Ok(decision)
    }

    fn note_oversize(&self) -> io::Result<()> {
        let mut state = self.inner.lock().map_err(|_| io::Error::other("ingest policy mutex poisoned"))?;
        state.stats.oversize_rejected = state.stats.oversize_rejected.saturating_add(1);
        maybe_report_overload(&self.config, &mut state);
        Ok(())
    }

    fn note_client_cap(&self) -> io::Result<()> {
        let mut state = self.inner.lock().map_err(|_| io::Error::other("ingest policy mutex poisoned"))?;
        state.stats.client_cap_rejected = state.stats.client_cap_rejected.saturating_add(1);
        maybe_report_overload(&self.config, &mut state);
        Ok(())
    }
}

fn maybe_report_overload(config: &IngestOverloadConfig, state: &mut IngestPolicyState) {
    if config.report_every_ms == 0 {
        return;
    }
    let now = Instant::now();
    if now < state.next_report_at {
        return;
    }
    eprintln!(
        "ljd ingest overload stats accepted={} priority-bypass={} rate-limited={} oversize-rejected={} client-cap-rejected={}",
        state.stats.accepted, state.stats.priority_bypass, state.stats.rate_limited, state.stats.oversize_rejected, state.stats.client_cap_rejected
    );
    state.next_report_at = now + Duration::from_millis(config.report_every_ms.max(1));
}

fn batch_priority_floor(floor: SeverityFloor) -> BatchPriority {
    match floor {
        SeverityFloor::Trace => BatchPriority::Trace,
        SeverityFloor::Debug => BatchPriority::Debug,
        SeverityFloor::Info => BatchPriority::Info,
        SeverityFloor::Warn => BatchPriority::Warn,
        SeverityFloor::Error => BatchPriority::Error,
        SeverityFloor::Fatal => BatchPriority::Fatal,
    }
}

pub fn serve(config: DaemonConfig) -> io::Result<()> {
    let spool_inner = Spool::open(config.config.storage.clone())?;
    let next_seq_seed = spool_inner.next_sequence_seed()?;
    let spool = Arc::new(SharedSpool::new(spool_inner));
    let ingest_policy = Arc::new(SharedIngestPolicy::new(config.config.ingest_overload));
    let next_seq = Arc::new(AtomicU64::new(next_seq_seed));

    let flush_spool = Arc::clone(&spool);
    thread::Builder::new().name("ljd-flush".to_string()).spawn(move || flush_loop(flush_spool))?;

    let replay_spool = Arc::clone(&spool);
    let replay_addr = config.config.replay_addr.clone();
    let replay_max_clients = config.config.replay_max_clients;
    let replay_client_timeout_ms = config.config.replay_client_timeout_ms;
    let wire_compression = config.config.wire_compression;
    let tls = config.config.tls.clone();

    let replay_thread = thread::Builder::new()
        .name("ljd-replay".to_string())
        .spawn(move || replay_loop(replay_addr, replay_spool, replay_max_clients, replay_client_timeout_ms, wire_compression, tls))?;

    eprintln!("ljd using config {}", config.config_path.display());
    ingest_loop(
        config.config.ingest_addr,
        config.config.ingest_protocol,
        config.config.ingest_tls,
        config.config.ingest_limits,
        config.config.ingest_plugin_path,
        ingest_policy,
        spool,
        next_seq,
    )?;
    replay_thread.join().map_err(|_| io::Error::other("replay listener thread panicked"))?
}

#[allow(clippy::too_many_arguments)]
fn ingest_loop(
    bind_addr: String, protocol: IngestProtocol, ingest_tls: IngestTlsConfig, ingest_limits: IngestLimits, plugin_path: Option<PathBuf>,
    ingest_policy: Arc<SharedIngestPolicy>, spool: Arc<SharedSpool>, next_seq: Arc<AtomicU64>,
) -> io::Result<()> {
    let limiter = Arc::new(ConnectionLimiter::new(ingest_limits.max_clients));
    match protocol {
        IngestProtocol::Plugin => {
            let path = plugin_path.ok_or_else(|| io::Error::other("ingest.plugin-path is required for plugin protocol"))?;
            let path = crate::plugin::resolve_ingest_plugin_path(&path);
            return crate::plugin::plugin_ingest_loop(&bind_addr, &path, spool, next_seq);
        }
        IngestProtocol::Wire => {
            let listener = TcpListener::bind(&bind_addr)?;
            eprintln!(
                "ljd ingest listening on {bind_addr} using wire protocol max-batch-bytes={} max-clients={}",
                ingest_limits.max_batch_bytes, ingest_limits.max_clients
            );

            for stream in listener.incoming() {
                let stream = stream?;
                let spool = Arc::clone(&spool);
                let ingest_policy = Arc::clone(&ingest_policy);
                let limiter = Arc::clone(&limiter);
                let max_batch_bytes = ingest_limits.max_batch_bytes;
                thread::Builder::new().name("ljd-ingest-client".to_string()).spawn(move || {
                    if let Err(err) = handle_ingest_client(stream, spool, ingest_policy, limiter, max_batch_bytes) {
                        eprintln!("ljd ingest client error: {err}");
                    }
                })?;
            }
        }
        IngestProtocol::OtlpHttp => {
            if ingest_tls.enable {
                return otlp_http_tls_loop(bind_addr, ingest_tls, ingest_limits, ingest_policy, spool, next_seq, limiter);
            }
            let server = Server::http(&bind_addr).map_err(|err| io::Error::other(err.to_string()))?;
            eprintln!("ljd ingest listening on http://{bind_addr}/v1/logs using otlp-http max-batch-bytes={}", ingest_limits.max_batch_bytes);

            for mut request in server.incoming_requests() {
                if request.method() != &Method::Post || request.url() != "/v1/logs" {
                    let response = Response::from_string("not found").with_status_code(StatusCode(404));
                    let _ = request.respond(response);
                    continue;
                }

                let mut body = Vec::with_capacity(ingest_limits.max_batch_bytes.min(8192));
                request.as_reader().take((ingest_limits.max_batch_bytes + 1) as u64).read_to_end(&mut body)?;
                if body.len() > ingest_limits.max_batch_bytes {
                    ingest_policy.note_oversize()?;
                    let response = Response::from_string("payload too large").with_status_code(StatusCode(413));
                    request.respond(response).map_err(|err| io::Error::other(err.to_string()))?;
                    continue;
                }
                match ExportLogsServiceRequest::decode(body.as_slice()) {
                    Ok(batch) => {
                        let decision = ingest_policy.decide(classify_otlp_batch_priority(&batch))?;
                        if matches!(decision, IngestDecision::RejectRateLimited) {
                            let response = Response::from_string("rate limit exceeded").with_status_code(StatusCode(429));
                            request.respond(response).map_err(|err| io::Error::other(err.to_string()))?;
                            continue;
                        }
                        let record = WireRecord {
                            record_type: logjet::RecordType::Logs,
                            seq: next_seq.fetch_add(1, Ordering::Relaxed),
                            ts_unix_ns: extract_batch_timestamp(&batch).unwrap_or_else(unix_time_nanos),
                            payload: body,
                        };
                        append_batch_record(&spool, record)?;

                        let response = Response::empty(200);
                        request.respond(response).map_err(|err| io::Error::other(err.to_string()))?;
                    }
                    Err(err) => {
                        let response = Response::from_string(format!("decode error: {err}")).with_status_code(StatusCode(400));
                        request.respond(response).map_err(|resp_err| io::Error::other(resp_err.to_string()))?;
                    }
                }
            }
        }
        IngestProtocol::OtlpGrpc => {
            let addr: SocketAddr =
                bind_addr.parse().map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, format!("invalid gRPC bind addr: {err}")))?;
            eprintln!(
                "ljd ingest listening on {}://{bind_addr} using otlp-grpc max-batch-bytes={} max-clients={}",
                if ingest_tls.enable { "grpcs" } else { "grpc" },
                ingest_limits.max_batch_bytes,
                ingest_limits.max_clients
            );

            let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().map_err(|err| io::Error::other(err.to_string()))?;
            let service = OtlpGrpcLogsService { spool, next_seq, ingest_policy };
            let grpc_tls = if ingest_tls.enable { Some(build_grpc_server_tls_config(&ingest_tls)?) } else { None };

            runtime.block_on(async move {
                let builder = tonic::transport::Server::builder();
                let builder =
                    if let Some(tls) = grpc_tls { builder.tls_config(tls).map_err(|err| io::Error::other(err.to_string()))? } else { builder };

                builder
                    .concurrency_limit_per_connection(ingest_limits.max_clients)
                    .add_service(LogsServiceServer::new(service).max_decoding_message_size(ingest_limits.max_batch_bytes))
                    .serve(addr)
                    .await
                    .map_err(|err| io::Error::other(err.to_string()))
            })?;
        }
    }

    Ok(())
}

fn otlp_http_tls_loop(
    bind_addr: String, ingest_tls: IngestTlsConfig, ingest_limits: IngestLimits, ingest_policy: Arc<SharedIngestPolicy>, spool: Arc<SharedSpool>,
    next_seq: Arc<AtomicU64>, limiter: Arc<ConnectionLimiter>,
) -> io::Result<()> {
    let listener = TcpListener::bind(&bind_addr)?;
    let tls_server = load_ingest_server_config(&ingest_tls)?;
    eprintln!(
        "ljd ingest listening on https://{bind_addr}/v1/logs using otlp-http max-batch-bytes={} max-clients={}",
        ingest_limits.max_batch_bytes, ingest_limits.max_clients
    );

    for stream in listener.incoming() {
        let stream = stream?;
        let spool = Arc::clone(&spool);
        let ingest_policy = Arc::clone(&ingest_policy);
        let next_seq = Arc::clone(&next_seq);
        let tls_server = tls_server.clone();
        let limiter = Arc::clone(&limiter);
        let max_batch_bytes = ingest_limits.max_batch_bytes;
        thread::Builder::new().name("ljd-otlp-http-tls-client".to_string()).spawn(move || {
            if let Err(err) = handle_otlp_http_tls_client(stream, tls_server, spool, ingest_policy, next_seq, limiter, max_batch_bytes) {
                eprintln!("ljd otlp-http tls client error: {err}");
            }
        })?;
    }

    Ok(())
}

fn handle_otlp_http_tls_client(
    stream: TcpStream, tls_server: Arc<ServerConfig>, spool: Arc<SharedSpool>, ingest_policy: Arc<SharedIngestPolicy>, next_seq: Arc<AtomicU64>,
    limiter: Arc<ConnectionLimiter>, max_batch_bytes: usize,
) -> io::Result<()> {
    let Some(_permit) = limiter.try_acquire() else {
        eprintln!("ljd ingest refused TLS client: ingest.max-clients reached");
        ingest_policy.note_client_cap()?;
        return Ok(());
    };
    let conn = ServerConnection::new(tls_server).map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))?;
    let mut transport = StreamOwned::new(conn, stream);
    let result = handle_otlp_http_transport(&mut transport, spool, ingest_policy, next_seq, max_batch_bytes);
    transport.conn.send_close_notify();
    let _ = transport.flush();
    result
}

fn handle_otlp_http_transport<T: Read + io::Write>(
    transport: &mut T, spool: Arc<SharedSpool>, ingest_policy: Arc<SharedIngestPolicy>, next_seq: Arc<AtomicU64>, max_batch_bytes: usize,
) -> io::Result<()> {
    let request = match read_http_request(transport, max_batch_bytes) {
        Ok(request) => request,
        Err(err) if err.kind() == io::ErrorKind::InvalidData && err.to_string() == "payload too large" => {
            ingest_policy.note_oversize()?;
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
            let decision = ingest_policy.decide(classify_otlp_batch_priority(&batch))?;
            if matches!(decision, IngestDecision::RejectRateLimited) {
                write_http_response(transport, 429, "rate limit exceeded")?;
                return Ok(());
            }
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

fn append_batch_record(spool: &Arc<SharedSpool>, record: WireRecord) -> io::Result<()> {
    {
        let mut inner = spool.spool.lock().map_err(|_| io::Error::other("spool mutex poisoned"))?;
        inner.append(record)?;
    }
    spool.notify_change()
}

/// Appends a record to the spool. Exposed for the plugin ingest path.
pub(crate) fn append_to_spool(spool: &Arc<SharedSpool>, record: WireRecord) -> io::Result<()> {
    append_batch_record(spool, record)
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
    let header_text =
        std::str::from_utf8(&buffer[..header_end - 4]).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "http header is not valid utf-8"))?;
    let mut lines = header_text.lines();
    let request_line = lines.next().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing http request line"))?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing http method"))?.to_string();
    let path = parts.next().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing http path"))?.to_string();

    let mut content_length = None;
    for line in lines {
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = Some(value.trim().parse::<usize>().map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid content-length"))?);
        }
    }

    let content_length = content_length.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing content-length"))?;
    if content_length > max_batch_bytes {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "payload too large"));
    }
    let mut body = Vec::with_capacity(content_length);
    transport.take(content_length as u64).read_to_end(&mut body)?;
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
        429 => "Too Many Requests",
        404 => "Not Found",
        503 => "Service Unavailable",
        _ => "Error",
    };
    write!(transport, "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", status, status_text, body.len(), body)?;
    transport.flush()
}

fn build_grpc_server_tls_config(ingest_tls: &IngestTlsConfig) -> io::Result<ServerTlsConfig> {
    let cert_file = ingest_tls
        .cert_file
        .as_deref()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "ingest.cert-file is required when ingest.tls-enable is true"))?;
    let key_file = ingest_tls
        .key_file
        .as_deref()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "ingest.key-file is required when ingest.tls-enable is true"))?;

    let identity = Identity::from_pem(fs::read(cert_file)?, fs::read(key_file)?);
    let mut tls = ServerTlsConfig::new().identity(identity);
    if ingest_tls.require_client_cert {
        let ca_file = ingest_tls
            .ca_file
            .as_deref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "ingest.ca-file is required when ingest.require-client-cert is true"))?;
        tls = tls.client_ca_root(Certificate::from_pem(fs::read(ca_file)?));
    }
    Ok(tls)
}

#[cfg(test)]
#[path = "../tests/unit/daemon_utst.rs"]
mod daemon_utst;

fn flush_loop(spool: Arc<SharedSpool>) -> io::Result<()> {
    loop {
        thread::sleep(Duration::from_millis(200));
        let mut inner = spool.spool.lock().map_err(|_| io::Error::other("spool mutex poisoned"))?;
        inner.flush_pending()?;
        inner.fsync_if_interval()?;
        drop(inner);
        spool.notify_change()?;
    }
}

fn replay_loop(
    bind_addr: String, spool: Arc<SharedSpool>, max_clients: usize, client_timeout_ms: u64, wire_compression: bool, tls: crate::config::TlsConfig,
) -> io::Result<()> {
    let listener = TcpListener::bind(&bind_addr)?;
    let limiter = Arc::new(ConnectionLimiter::new(max_clients));
    let tls_server = if tls.enable {
        eprintln!("ljd replay TLS enabled on {bind_addr}");
        Some(load_server_config(&tls)?)
    } else {
        None
    };
    eprintln!("ljd replay listening on {bind_addr} max-clients={max_clients} client-timeout-ms={client_timeout_ms}");

    for stream in listener.incoming() {
        let stream = stream?;
        let timeout = Duration::from_millis(client_timeout_ms);
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        let spool = Arc::clone(&spool);
        let tls_server = tls_server.clone();
        let limiter = Arc::clone(&limiter);
        thread::Builder::new().name("ljd-replay-client".to_string()).spawn(move || {
            if let Err(err) = handle_replay_client(stream, spool, tls_server, limiter, wire_compression) {
                eprintln!("ljd replay client error: {err}");
            }
        })?;
    }

    Ok(())
}

fn handle_ingest_client(
    stream: TcpStream, spool: Arc<SharedSpool>, ingest_policy: Arc<SharedIngestPolicy>, limiter: Arc<ConnectionLimiter>, max_batch_bytes: usize,
) -> io::Result<()> {
    let Some(_permit) = limiter.try_acquire() else {
        eprintln!("ljd ingest refused wire client: ingest.max-clients reached");
        ingest_policy.note_client_cap()?;
        return Ok(());
    };
    let peer = stream.peer_addr().ok();
    let mut reader = BufReader::new(stream);

    while let Some(record) = read_record_with_limit(&mut reader, max_batch_bytes)? {
        if matches!(ingest_policy.decide(BatchPriority::Unknown)?, IngestDecision::RejectRateLimited) {
            eprintln!("ljd ingest dropped wire record seq={} because ingest rate limit was exceeded", record.seq);
            continue;
        }
        append_batch_record(&spool, record)?;
    }

    if let Some(peer) = peer {
        eprintln!("ljd ingest disconnected: {peer}");
    }
    Ok(())
}

struct ConnectionLimiter {
    max_clients: usize,
    active_clients: std::sync::atomic::AtomicUsize,
}

impl ConnectionLimiter {
    fn new(max_clients: usize) -> Self {
        Self { max_clients, active_clients: std::sync::atomic::AtomicUsize::new(0) }
    }

    fn try_acquire(self: &Arc<Self>) -> Option<ConnectionPermit> {
        loop {
            let current = self.active_clients.load(Ordering::Relaxed);
            if current >= self.max_clients {
                return None;
            }
            if self.active_clients.compare_exchange(current, current + 1, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
                return Some(ConnectionPermit { limiter: Arc::clone(self) });
            }
        }
    }
}

struct ConnectionPermit {
    limiter: Arc<ConnectionLimiter>,
}

impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        self.limiter.active_clients.fetch_sub(1, Ordering::AcqRel);
    }
}

fn handle_replay_client(
    stream: TcpStream, spool: Arc<SharedSpool>, tls_server: Option<Arc<ServerConfig>>, limiter: Arc<ConnectionLimiter>, wire_compression: bool,
) -> io::Result<()> {
    let Some(_permit) = limiter.try_acquire() else {
        eprintln!("ljd replay refused client: replay.max-clients reached");
        return Ok(());
    };
    if let Some(server_config) = tls_server {
        let conn = ServerConnection::new(server_config).map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))?;
        let mut transport = StreamOwned::new(conn, stream);
        return handle_replay_transport(&mut transport, spool, wire_compression);
    }

    let mut transport = stream;
    handle_replay_transport(&mut transport, spool, wire_compression)
}

fn handle_replay_transport<T: io::Read + io::Write>(transport: &mut T, spool: Arc<SharedSpool>, wire_compression: bool) -> io::Result<()> {
    let hello = {
        let spool = spool.spool.lock().map_err(|_| io::Error::other("spool mutex poisoned"))?;
        let (first_seq, last_seq) = spool.sequence_bounds()?.unwrap_or((0, 0));
        ReplayHello { stream_id: spool.stream_id(), first_seq, last_seq }
    };
    write_replay_hello(transport, &hello)?;
    transport.flush()?;

    let request = read_replay_request(transport)?;
    let (mut cursor, mut seen_generation) = {
        let spool_guard = spool.spool.lock().map_err(|_| io::Error::other("spool mutex poisoned"))?;
        (spool_guard.replay_cursor_after(request.from_seq)?, spool.current_generation()?)
    };

    eprintln!("ljd replay client requested records after seq={} mode={}", request.from_seq, if request.consume { "drain" } else { "keep" });

    if request.consume {
        return handle_replay_transport_drain(transport, spool, &mut cursor, &mut seen_generation, wire_compression);
    }

    loop {
        let mut sent_any = false;
        loop {
            let next_record = {
                let spool = spool.spool.lock().map_err(|_| io::Error::other("spool mutex poisoned"))?;
                spool.next_for_cursor(&mut cursor)?
            };

            let Some(record) = next_record else {
                break;
            };

            write_record(transport, &record, wire_compression)?;
            sent_any = true;
        }

        if sent_any {
            transport.flush()?;
            continue;
        }

        spool.wait_for_change(&mut seen_generation)?;
    }
}

fn handle_replay_transport_drain<T: io::Read + io::Write>(
    transport: &mut T, spool: Arc<SharedSpool>, cursor: &mut crate::spool::ReplayCursor, seen_generation: &mut u64, wire_compression: bool,
) -> io::Result<()> {
    loop {
        let next_record = {
            let spool = spool.spool.lock().map_err(|_| io::Error::other("spool mutex poisoned"))?;
            spool.next_for_cursor(cursor)?
        };

        let Some(record) = next_record else {
            spool.wait_for_change(seen_generation)?;
            continue;
        };

        write_record(transport, &record, wire_compression)?;
        transport.flush()?;

        let ack = read_replay_ack(transport)?;
        if ack.ack_seq != record.seq {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("replay ack out of order: expected {} got {}", record.seq, ack.ack_seq)));
        }

        {
            let mut spool = spool.spool.lock().map_err(|_| io::Error::other("spool mutex poisoned"))?;
            spool.consume_through(ack.ack_seq)?;
        }
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

fn classify_otlp_batch_priority(batch: &ExportLogsServiceRequest) -> BatchPriority {
    let mut highest = BatchPriority::Unknown;
    for resource_logs in &batch.resource_logs {
        for scope_logs in &resource_logs.scope_logs {
            for record in &scope_logs.log_records {
                highest = highest.max(priority_from_severity_number(record.severity_number));
            }
        }
    }
    highest
}

fn priority_from_severity_number(severity_number: i32) -> BatchPriority {
    match severity_number {
        21..=24 => BatchPriority::Fatal,
        17..=20 => BatchPriority::Error,
        13..=16 => BatchPriority::Warn,
        9..=12 => BatchPriority::Info,
        5..=8 => BatchPriority::Debug,
        1..=4 => BatchPriority::Trace,
        _ => BatchPriority::Unknown,
    }
}

fn unix_time_nanos() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos() as u64
}

#[derive(Clone)]
struct OtlpGrpcLogsService {
    spool: Arc<SharedSpool>,
    next_seq: Arc<AtomicU64>,
    ingest_policy: Arc<SharedIngestPolicy>,
}

#[tonic::async_trait]
impl LogsService for OtlpGrpcLogsService {
    async fn export(&self, request: Request<ExportLogsServiceRequest>) -> Result<GrpcResponse<ExportLogsServiceResponse>, Status> {
        let batch = request.into_inner();
        match self.ingest_policy.decide(classify_otlp_batch_priority(&batch)).map_err(|err| Status::internal(err.to_string()))? {
            IngestDecision::Accept | IngestDecision::AcceptPriorityBypass => {}
            IngestDecision::RejectRateLimited => {
                return Err(Status::resource_exhausted("ingest rate limit exceeded"));
            }
        }
        let payload = batch.encode_to_vec();
        let record = WireRecord {
            record_type: logjet::RecordType::Logs,
            seq: self.next_seq.fetch_add(1, Ordering::Relaxed),
            ts_unix_ns: extract_batch_timestamp(&batch).unwrap_or_else(unix_time_nanos),
            payload,
        };

        append_batch_record(&self.spool, record).map_err(|err| Status::internal(err.to_string()))?;

        Ok(GrpcResponse::new(ExportLogsServiceResponse { partial_success: None }))
    }
}
