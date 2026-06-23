use std::fs;
use std::io::{self, BufReader, Read};
use std::net::SocketAddr;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response as HyperResponse, StatusCode};
use hyper_util::rt::TokioIo;
use opentelemetry_proto::tonic::collector::logs::v1::{
    ExportLogsServiceRequest, ExportLogsServiceResponse,
    logs_service_server::{LogsService, LogsServiceServer},
};
use opentelemetry_proto::tonic::collector::metrics::v1::{
    ExportMetricsServiceRequest, ExportMetricsServiceResponse,
    metrics_service_server::{MetricsService, MetricsServiceServer},
};
use opentelemetry_proto::tonic::collector::trace::v1::{
    ExportTraceServiceRequest, ExportTraceServiceResponse,
    trace_service_server::{TraceService, TraceServiceServer},
};
use prost::Message;
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use tokio::net::TcpListener as TokioTcpListener;
use tokio_rustls::TlsAcceptor;
use tonic::transport::{Certificate, Identity, ServerTlsConfig};
use tonic::{Request as TonicRequest, Response as GrpcResponse, Status};

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
        config.config.ingest_plugin_dir,
        config.config.ingest_plugin_name,
        config.config.ingest_plugin_env,
        ingest_policy,
        spool,
        next_seq,
    )?;
    replay_thread.join().map_err(|_| io::Error::other("replay listener thread panicked"))?
}

#[allow(clippy::too_many_arguments)]
fn ingest_loop(
    bind_addr: String, protocol: IngestProtocol, ingest_tls: IngestTlsConfig, ingest_limits: IngestLimits, plugin_path: Option<PathBuf>,
    plugin_dir: Option<PathBuf>, plugin_name: Option<String>, plugin_env: Vec<String>, ingest_policy: Arc<SharedIngestPolicy>,
    spool: Arc<SharedSpool>, next_seq: Arc<AtomicU64>,
) -> io::Result<()> {
    let limiter = Arc::new(ConnectionLimiter::new(ingest_limits.max_clients));
    match protocol {
        IngestProtocol::Plugin => {
            let path = crate::plugin::resolve_ingest_plugin(plugin_path.as_deref(), plugin_dir.as_deref(), plugin_name.as_deref())?;
            eprintln!("ljd ingest plugin selected name={} path={}", crate::plugin::ingest_plugin_label(&path), path.display());
            return crate::plugin::plugin_ingest_loop(&bind_addr, &path, &plugin_env, spool, next_seq);
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
            return otlp_http_async_loop(bind_addr, ingest_tls, ingest_limits, ingest_policy, spool, next_seq, limiter);
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
            let logs_service =
                OtlpGrpcLogsService { spool: Arc::clone(&spool), next_seq: Arc::clone(&next_seq), ingest_policy: Arc::clone(&ingest_policy) };
            let metrics_service =
                OtlpGrpcMetricsService { spool: Arc::clone(&spool), next_seq: Arc::clone(&next_seq), ingest_policy: Arc::clone(&ingest_policy) };
            let traces_service = OtlpGrpcTracesService { spool, next_seq, ingest_policy };
            let grpc_tls = if ingest_tls.enable { Some(build_grpc_server_tls_config(&ingest_tls)?) } else { None };

            runtime.block_on(async move {
                let builder = tonic::transport::Server::builder();
                let builder =
                    if let Some(tls) = grpc_tls { builder.tls_config(tls).map_err(|err| io::Error::other(err.to_string()))? } else { builder };

                builder
                    .concurrency_limit_per_connection(ingest_limits.max_clients)
                    .add_service(LogsServiceServer::new(logs_service).max_decoding_message_size(ingest_limits.max_batch_bytes))
                    .add_service(MetricsServiceServer::new(metrics_service).max_decoding_message_size(ingest_limits.max_batch_bytes))
                    .add_service(TraceServiceServer::new(traces_service).max_decoding_message_size(ingest_limits.max_batch_bytes))
                    .serve(addr)
                    .await
                    .map_err(|err| io::Error::other(err.to_string()))
            })?;
        }
    }

    Ok(())
}

fn otlp_http_async_loop(
    bind_addr: String, ingest_tls: IngestTlsConfig, ingest_limits: IngestLimits, ingest_policy: Arc<SharedIngestPolicy>, spool: Arc<SharedSpool>,
    next_seq: Arc<AtomicU64>, limiter: Arc<ConnectionLimiter>,
) -> io::Result<()> {
    let max_batch_bytes = ingest_limits.max_batch_bytes;
    let max_clients = ingest_limits.max_clients;
    let tls_acceptor = if ingest_tls.enable {
        let server_config = load_ingest_server_config(&ingest_tls)?;
        Some(TlsAcceptor::from(server_config))
    } else {
        None
    };

    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().map_err(|err| io::Error::other(err.to_string()))?;
    let scheme = if ingest_tls.enable { "https" } else { "http" };
    eprintln!(
        "ljd ingest listening on {scheme}://{bind_addr}/v1/logs /v1/metrics /v1/traces using otlp-http max-batch-bytes={max_batch_bytes} max-clients={max_clients}"
    );

    runtime.block_on(async move {
        let listener = TokioTcpListener::bind(&bind_addr).await.map_err(|err| io::Error::other(err.to_string()))?;

        loop {
            let (stream, _peer) = match listener.accept().await {
                Ok(accepted) => accepted,
                Err(err) => {
                    eprintln!("ljd ingest accept error: {err}");
                    continue;
                }
            };

            let spool = Arc::clone(&spool);
            let ingest_policy = Arc::clone(&ingest_policy);
            let next_seq = Arc::clone(&next_seq);
            let limiter = Arc::clone(&limiter);
            let tls_acceptor = tls_acceptor.clone();

            tokio::spawn(async move {
                if let Err(err) = serve_otlp_http_connection(stream, tls_acceptor, spool, ingest_policy, next_seq, limiter, max_batch_bytes).await {
                    eprintln!("ljd ingest client error: {err}");
                }
            });
        }
    })
}

async fn serve_otlp_http_connection(
    stream: tokio::net::TcpStream, tls_acceptor: Option<TlsAcceptor>, spool: Arc<SharedSpool>, ingest_policy: Arc<SharedIngestPolicy>,
    next_seq: Arc<AtomicU64>, limiter: Arc<ConnectionLimiter>, max_batch_bytes: usize,
) -> io::Result<()> {
    let Some(_permit) = limiter.try_acquire() else {
        eprintln!("ljd ingest refused client: ingest.max-clients reached");
        ingest_policy.note_client_cap()?;
        return Ok(());
    };

    let svc =
        service_fn(move |req| handle_otlp_http_request(req, Arc::clone(&spool), Arc::clone(&ingest_policy), Arc::clone(&next_seq), max_batch_bytes));

    if let Some(acceptor) = tls_acceptor {
        let tls_stream = acceptor.accept(stream).await.map_err(|err| io::Error::other(err.to_string()))?;
        let _ = http1::Builder::new().serve_connection(TokioIo::new(tls_stream), svc).await;
    } else {
        let _ = http1::Builder::new().serve_connection(TokioIo::new(stream), svc).await;
    }

    Ok(())
}

async fn handle_otlp_http_request<B>(
    req: Request<B>, spool: Arc<SharedSpool>, ingest_policy: Arc<SharedIngestPolicy>, next_seq: Arc<AtomicU64>, max_batch_bytes: usize,
) -> Result<HyperResponse<Full<Bytes>>, io::Error>
where
    B: hyper::body::Body<Data = Bytes>,
    B::Error: std::fmt::Display,
{
    if req.method() != Method::POST || !matches!(req.uri().path(), "/v1/logs" | "/v1/metrics" | "/v1/traces") {
        return Ok(HyperResponse::builder().status(StatusCode::NOT_FOUND).body(Full::new(Bytes::from("not found"))).unwrap());
    }
    let is_metrics = req.uri().path() == "/v1/metrics";
    let is_traces = req.uri().path() == "/v1/traces";

    let content_encoding = req.headers().get("content-encoding").and_then(|v| v.to_str().ok()).map(|s| s.to_string());

    let (parts, body) = req.into_parts();

    // Check content-length header first for early rejection.
    if let Some(len) = parts.headers.get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<usize>().ok())
        && len > max_batch_bytes
    {
        ingest_policy.note_oversize()?;
        return Ok(HyperResponse::builder().status(StatusCode::PAYLOAD_TOO_LARGE).body(Full::new(Bytes::from("payload too large"))).unwrap());
    }

    let collected = body.collect().await.map_err(|err| io::Error::other(format!("failed to read request body: {err}")))?;
    let body_bytes = collected.to_bytes();

    if body_bytes.len() > max_batch_bytes {
        ingest_policy.note_oversize()?;
        return Ok(HyperResponse::builder().status(StatusCode::PAYLOAD_TOO_LARGE).body(Full::new(Bytes::from("payload too large"))).unwrap());
    }

    let body_vec = match maybe_decompress_body(body_bytes.to_vec(), content_encoding.as_deref()) {
        Ok(b) => b,
        Err(err) => {
            return Ok(HyperResponse::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Full::new(Bytes::from(format!("decompression error: {err}"))))
                .unwrap());
        }
    };

    if is_metrics {
        match ExportMetricsServiceRequest::decode(body_vec.as_slice()) {
            Ok(batch) => {
                let decision = ingest_policy.decide(BatchPriority::Unknown)?;
                if matches!(decision, IngestDecision::RejectRateLimited) {
                    return Ok(HyperResponse::builder()
                        .status(StatusCode::TOO_MANY_REQUESTS)
                        .body(Full::new(Bytes::from("rate limit exceeded")))
                        .unwrap());
                }
                let record = WireRecord {
                    record_type: logjet::RecordType::Metrics,
                    seq: next_seq.fetch_add(1, Ordering::Relaxed),
                    ts_unix_ns: extract_batch_timestamp_metrics(&batch).unwrap_or_else(unix_time_nanos),
                    payload: body_vec,
                };
                append_batch_record(&spool, record)?;
                Ok(HyperResponse::builder().status(StatusCode::OK).body(Full::new(Bytes::new())).unwrap())
            }
            Err(err) => {
                Ok(HyperResponse::builder().status(StatusCode::BAD_REQUEST).body(Full::new(Bytes::from(format!("decode error: {err}")))).unwrap())
            }
        }
    } else if is_traces {
        match ExportTraceServiceRequest::decode(body_vec.as_slice()) {
            Ok(batch) => {
                let decision = ingest_policy.decide(BatchPriority::Unknown)?;
                if matches!(decision, IngestDecision::RejectRateLimited) {
                    return Ok(HyperResponse::builder()
                        .status(StatusCode::TOO_MANY_REQUESTS)
                        .body(Full::new(Bytes::from("rate limit exceeded")))
                        .unwrap());
                }
                let record = WireRecord {
                    record_type: logjet::RecordType::Traces,
                    seq: next_seq.fetch_add(1, Ordering::Relaxed),
                    ts_unix_ns: extract_batch_timestamp_traces(&batch).unwrap_or_else(unix_time_nanos),
                    payload: body_vec,
                };
                append_batch_record(&spool, record)?;
                Ok(HyperResponse::builder().status(StatusCode::OK).body(Full::new(Bytes::new())).unwrap())
            }
            Err(err) => {
                Ok(HyperResponse::builder().status(StatusCode::BAD_REQUEST).body(Full::new(Bytes::from(format!("decode error: {err}")))).unwrap())
            }
        }
    } else {
        match ExportLogsServiceRequest::decode(body_vec.as_slice()) {
            Ok(batch) => {
                let decision = ingest_policy.decide(classify_otlp_batch_priority(&batch))?;
                if matches!(decision, IngestDecision::RejectRateLimited) {
                    return Ok(HyperResponse::builder()
                        .status(StatusCode::TOO_MANY_REQUESTS)
                        .body(Full::new(Bytes::from("rate limit exceeded")))
                        .unwrap());
                }
                let record = WireRecord {
                    record_type: logjet::RecordType::Logs,
                    seq: next_seq.fetch_add(1, Ordering::Relaxed),
                    ts_unix_ns: extract_batch_timestamp(&batch).unwrap_or_else(unix_time_nanos),
                    payload: body_vec,
                };
                append_batch_record(&spool, record)?;
                Ok(HyperResponse::builder().status(StatusCode::OK).body(Full::new(Bytes::new())).unwrap())
            }
            Err(err) => {
                Ok(HyperResponse::builder().status(StatusCode::BAD_REQUEST).body(Full::new(Bytes::from(format!("decode error: {err}")))).unwrap())
            }
        }
    }
}

fn append_batch_record(spool: &Arc<SharedSpool>, record: WireRecord) -> io::Result<()> {
    {
        let mut inner = spool.spool.lock().map_err(|_| io::Error::other("spool mutex poisoned"))?;
        inner.append(record)?;
    }
    spool.notify_change()
}

pub(crate) fn append_to_spool(spool: &Arc<SharedSpool>, record: WireRecord) -> io::Result<()> {
    append_batch_record(spool, record)
}

fn maybe_decompress_body(body: Vec<u8>, encoding: Option<&str>) -> io::Result<Vec<u8>> {
    match encoding {
        Some("gzip") | Some("x-gzip") => {
            use flate2::read::GzDecoder;
            let mut decoder = GzDecoder::new(body.as_slice());
            let mut out = Vec::with_capacity(body.len().saturating_mul(3));
            decoder.read_to_end(&mut out).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, format!("gzip decompression failed: {err}")))?;
            Ok(out)
        }
        _ => Ok(body),
    }
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

fn extract_batch_timestamp_metrics(batch: &ExportMetricsServiceRequest) -> Option<u64> {
    use opentelemetry_proto::tonic::metrics::v1::metric::Data;
    for resource_metrics in &batch.resource_metrics {
        for scope_metrics in &resource_metrics.scope_metrics {
            for metric in &scope_metrics.metrics {
                let ts = match metric.data.as_ref()? {
                    Data::Gauge(g) => g.data_points.iter().find_map(|dp| if dp.time_unix_nano != 0 { Some(dp.time_unix_nano) } else { None }),
                    Data::Sum(s) => s.data_points.iter().find_map(|dp| if dp.time_unix_nano != 0 { Some(dp.time_unix_nano) } else { None }),
                    Data::Histogram(h) => h.data_points.iter().find_map(|dp| if dp.time_unix_nano != 0 { Some(dp.time_unix_nano) } else { None }),
                    Data::ExponentialHistogram(eh) => {
                        eh.data_points.iter().find_map(|dp| if dp.time_unix_nano != 0 { Some(dp.time_unix_nano) } else { None })
                    }
                    Data::Summary(s) => s.data_points.iter().find_map(|dp| if dp.time_unix_nano != 0 { Some(dp.time_unix_nano) } else { None }),
                };
                if ts.is_some() {
                    return ts;
                }
            }
        }
    }
    None
}

fn extract_batch_timestamp_traces(batch: &ExportTraceServiceRequest) -> Option<u64> {
    for resource_spans in &batch.resource_spans {
        for scope_spans in &resource_spans.scope_spans {
            for span in &scope_spans.spans {
                if span.start_time_unix_nano != 0 {
                    return Some(span.start_time_unix_nano);
                }
            }
        }
    }
    None
}

#[derive(Clone)]
struct OtlpGrpcTracesService {
    spool: Arc<SharedSpool>,
    next_seq: Arc<AtomicU64>,
    ingest_policy: Arc<SharedIngestPolicy>,
}

#[tonic::async_trait]
impl TraceService for OtlpGrpcTracesService {
    async fn export(&self, request: TonicRequest<ExportTraceServiceRequest>) -> Result<GrpcResponse<ExportTraceServiceResponse>, Status> {
        let batch = request.into_inner();
        // Traces have no severity concept in OTLP, so they always classify as
        // BatchPriority::Unknown (lowest priority). See HTTP ingest comment above
        // for the full rationale on metrics/traces rate-limiting policy.
        match self.ingest_policy.decide(BatchPriority::Unknown).map_err(|err| Status::internal(err.to_string()))? {
            IngestDecision::Accept | IngestDecision::AcceptPriorityBypass => {}
            IngestDecision::RejectRateLimited => {
                return Err(Status::resource_exhausted("ingest rate limit exceeded"));
            }
        }
        let payload = batch.encode_to_vec();
        let record = WireRecord {
            record_type: logjet::RecordType::Traces,
            seq: self.next_seq.fetch_add(1, Ordering::Relaxed),
            ts_unix_ns: extract_batch_timestamp_traces(&batch).unwrap_or_else(unix_time_nanos),
            payload,
        };

        append_batch_record(&self.spool, record).map_err(|err| Status::internal(err.to_string()))?;

        Ok(GrpcResponse::new(ExportTraceServiceResponse { partial_success: None }))
    }
}

#[derive(Clone)]
struct OtlpGrpcMetricsService {
    spool: Arc<SharedSpool>,
    next_seq: Arc<AtomicU64>,
    ingest_policy: Arc<SharedIngestPolicy>,
}

#[tonic::async_trait]
impl MetricsService for OtlpGrpcMetricsService {
    async fn export(&self, request: TonicRequest<ExportMetricsServiceRequest>) -> Result<GrpcResponse<ExportMetricsServiceResponse>, Status> {
        let batch = request.into_inner();
        // Metrics have no severity concept in OTLP, so they always classify as
        // BatchPriority::Unknown (lowest priority). See HTTP ingest comment above
        // for the full rationale on metrics/traces rate-limiting policy.
        match self.ingest_policy.decide(BatchPriority::Unknown).map_err(|err| Status::internal(err.to_string()))? {
            IngestDecision::Accept | IngestDecision::AcceptPriorityBypass => {}
            IngestDecision::RejectRateLimited => {
                return Err(Status::resource_exhausted("ingest rate limit exceeded"));
            }
        }
        let payload = batch.encode_to_vec();
        let record = WireRecord {
            record_type: logjet::RecordType::Metrics,
            seq: self.next_seq.fetch_add(1, Ordering::Relaxed),
            ts_unix_ns: extract_batch_timestamp_metrics(&batch).unwrap_or_else(unix_time_nanos),
            payload,
        };

        append_batch_record(&self.spool, record).map_err(|err| Status::internal(err.to_string()))?;

        Ok(GrpcResponse::new(ExportMetricsServiceResponse { partial_success: None }))
    }
}

#[derive(Clone)]
struct OtlpGrpcLogsService {
    spool: Arc<SharedSpool>,
    next_seq: Arc<AtomicU64>,
    ingest_policy: Arc<SharedIngestPolicy>,
}

#[tonic::async_trait]
impl LogsService for OtlpGrpcLogsService {
    async fn export(&self, request: TonicRequest<ExportLogsServiceRequest>) -> Result<GrpcResponse<ExportLogsServiceResponse>, Status> {
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
