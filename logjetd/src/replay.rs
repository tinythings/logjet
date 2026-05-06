use std::fs::{self, File};
use std::io::{self, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use logjet::{LogjetReader, ReaderConfig, RecordType};
use opentelemetry_proto::tonic::collector::logs::v1::{ExportLogsServiceRequest, logs_service_client::LogsServiceClient};
use prost::Message;
use rustls::{ClientConfig, ClientConnection, StreamOwned};
use tokio::runtime::{Builder, Runtime};
use tonic::Request;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint, Identity};

use crate::config::{BackpressureConfig, BackpressureMode, CollectorConfig, TlsConfig, UpstreamConfig, UpstreamMode};
use crate::protocol::{ReplayAck, ReplayHello, ReplayRequest, read_record, read_replay_hello, write_replay_ack, write_replay_request};
use crate::spool::list_named_segments;
use crate::tls::{authority_host, load_client_config, load_collector_client_config, parse_collector_server_name, parse_server_name};

pub fn replay_path_to_otlp_http(path: &Path, name: &str, collector: &CollectorConfig) -> io::Result<u64> {
    let mut sent = 0u64;
    let endpoints = parse_collector_endpoints(collector)?;
    let mut conn = MultiCollectorConnection::connect(&endpoints, Duration::from_millis(collector.timeout_ms), collector)?;
    let mut batcher = OtlpBatcher::new(collector.batch_size, collector.batch_timeout_ms);

    for segment in list_named_segments(path, name)? {
        let file = File::open(&segment.path)?;
        let mut reader = LogjetReader::with_config(BufReader::new(file), ReaderConfig::default());

        while let Some(record) = reader.next_record().map_err(to_io_error)? {
            if record.record_type == RecordType::Logs {
                batcher.add(&record.payload, &mut conn)?;
                sent = sent.saturating_add(1);
            }
        }
    }

    batcher.flush(&mut conn)?;
    Ok(sent)
}

pub fn validate_replay_path(path: &Path, name: &str) -> io::Result<Vec<PathBuf>> {
    let segments = list_named_segments(path, name)?;
    Ok(segments.into_iter().map(|segment| segment.path).collect())
}

pub fn bridge_wire_to_otlp_http(
    source: &str, collector: &CollectorConfig, backpressure: &BackpressureConfig, upstream: &UpstreamConfig, tls: &TlsConfig,
) -> io::Result<()> {
    let endpoints = parse_collector_endpoints(collector)?;
    let connect_timeout = Duration::from_millis(upstream.connect_timeout_ms);
    let retry_delay = Duration::from_millis(upstream.retry_ms);
    let tls_client = if tls.enable { Some(load_client_config(tls)?) } else { None };
    let collector_transport = CollectorTransport {
        timeout: Duration::from_millis(collector.timeout_ms),
        backpressure_enabled: backpressure.enabled,
        backpressure_mode: backpressure.mode,
        max_buffered_records: backpressure.max_buffered_records,
        endpoints,
        collector: collector.clone(),
        upstream_mode: upstream.mode,
    };
    let mut state = read_bridge_state(upstream.state_file.as_deref())?;
    if let Some(path) = upstream.state_file.as_deref() {
        eprintln!(
            "bridge resume state file {} loaded seq={} stream-id={}",
            path.display(),
            state.last_seq,
            state.stream_id.map(|value| value.to_string()).unwrap_or_else(|| "unset".to_string())
        );
    }

    loop {
        match bridge_once(source, connect_timeout, &mut state, upstream.state_file.as_deref(), tls, tls_client.clone(), &collector_transport) {
            Ok(()) => {
                eprintln!("bridge source {source} closed after seq={}; reconnecting in {} ms", state.last_seq, upstream.retry_ms);
            }
            Err(err) => {
                eprintln!("bridge source {source} error after seq={}: {err}; reconnecting in {} ms", state.last_seq, upstream.retry_ms);
            }
        }
        thread::sleep(retry_delay);
    }
}

fn bridge_once(
    source: &str, connect_timeout: Duration, state: &mut BridgeState, state_file: Option<&Path>, tls: &TlsConfig,
    tls_client: Option<Arc<ClientConfig>>, collector_transport: &CollectorTransport,
) -> io::Result<()> {
    let stream = connect_with_timeout(source, connect_timeout)?;
    stream.set_read_timeout(None)?;
    stream.set_write_timeout(Some(connect_timeout))?;

    if let Some(client_config) = tls_client {
        let server_name = parse_server_name(tls, source)?;
        let conn = ClientConnection::new(client_config, server_name).map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))?;
        let mut transport = StreamOwned::new(conn, stream);
        return bridge_transport(source, state, state_file, &mut transport, collector_transport);
    }

    let mut transport = stream;
    bridge_transport(source, state, state_file, &mut transport, collector_transport)
}

fn bridge_transport<T: io::Read + io::Write>(
    source: &str, state: &mut BridgeState, state_file: Option<&Path>, transport: &mut T, collector_transport: &CollectorTransport,
) -> io::Result<()> {
    let hello = read_replay_hello(transport)?;
    reconcile_bridge_state(source, state, &hello)?;
    write_bridge_state(state_file, state)?;

    let consume = collector_transport.upstream_mode == UpstreamMode::Drain;
    write_replay_request(transport, &ReplayRequest { from_seq: state.last_seq, consume })?;
    transport.flush()?;
    eprintln!(
        "bridge connected to {} and requested records after seq={} mode={} backpressure={}",
        source,
        state.last_seq,
        if consume { "drain" } else { "keep" },
        collector_transport.describe_backpressure()
    );

    if !collector_transport.backpressure_enabled {
        let mut conn = collector_transport.open_connection()?;
        while let Some(record) = read_record(transport)? {
            if record.record_type == RecordType::Logs {
                conn.post(&record.payload)?;
            }
            commit_record(transport, state, state_file, consume, record.seq)?;
        }
        return Ok(());
    }

    let (task_tx, task_rx) = mpsc::sync_channel(collector_transport.max_buffered_records);
    let (result_tx, result_rx) = mpsc::channel();
    let worker_transport = collector_transport.clone();
    let exporter = thread::spawn(move || export_worker(worker_transport, task_rx, result_tx));
    let mut pending = std::collections::VecDeque::new();

    while let Some(record) = read_record(transport)? {
        flush_ready_results(transport, state, state_file, consume, &mut pending, &result_rx, false)?;

        if record.record_type != RecordType::Logs {
            commit_record(transport, state, state_file, consume, record.seq)?;
            continue;
        }

        let seq = record.seq;
        match enqueue_export_task(&task_tx, collector_transport, ExportTask { seq, payload: record.payload }) {
            Ok(EnqueueOutcome::Queued) => pending.push_back(PendingExport::Queued(seq)),
            Ok(EnqueueOutcome::DroppedNewest) => pending.push_back(PendingExport::Dropped(seq)),
            Err(err) => {
                drop(task_tx);
                let _ = exporter.join();
                return Err(err);
            }
        }
    }

    drop(task_tx);
    flush_ready_results(transport, state, state_file, consume, &mut pending, &result_rx, true)?;
    match exporter.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(err),
        Err(_) => Err(io::Error::other("collector export worker panicked")),
    }
}

fn enqueue_export_task(
    task_tx: &mpsc::SyncSender<ExportTask>, collector_transport: &CollectorTransport, task: ExportTask,
) -> io::Result<EnqueueOutcome> {
    match collector_transport.backpressure_mode {
        BackpressureMode::Block => task_tx
            .send(task)
            .map(|()| EnqueueOutcome::Queued)
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "collector export worker stopped")),
        BackpressureMode::Disconnect => match task_tx.try_send(task) {
            Ok(()) => Ok(EnqueueOutcome::Queued),
            Err(mpsc::TrySendError::Full(_)) => Err(io::Error::new(io::ErrorKind::TimedOut, "collector export buffer is full; disconnecting bridge")),
            Err(mpsc::TrySendError::Disconnected(_)) => Err(io::Error::new(io::ErrorKind::BrokenPipe, "collector export worker stopped")),
        },
        BackpressureMode::DropNewest => match task_tx.try_send(task) {
            Ok(()) => Ok(EnqueueOutcome::Queued),
            Err(mpsc::TrySendError::Full(task)) => {
                eprintln!("bridge dropping seq={} because collector export buffer is full (mode=drop-newest)", task.seq);
                Ok(EnqueueOutcome::DroppedNewest)
            }
            Err(mpsc::TrySendError::Disconnected(_)) => Err(io::Error::new(io::ErrorKind::BrokenPipe, "collector export worker stopped")),
        },
    }
}

fn export_worker(
    collector_transport: CollectorTransport, task_rx: mpsc::Receiver<ExportTask>, result_tx: mpsc::Sender<ExportResult>,
) -> io::Result<()> {
    let mut conn = collector_transport.open_connection()?;
    let mut batcher = OtlpBatcher::new(collector_transport.collector.batch_size, collector_transport.collector.batch_timeout_ms);
    let recv_timeout = Duration::from_millis(collector_transport.collector.batch_timeout_ms.max(50));
    loop {
        let task = match task_rx.recv_timeout(recv_timeout) {
            Ok(task) => task,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                batcher.flush_if_expired(&mut conn)?;
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let outcome = batcher.add(&task.payload, &mut conn).map(|()| ExportOutcome::Delivered);
        batcher.flush_if_expired(&mut conn)?;
        let failed = outcome.is_err();
        if result_tx.send(ExportResult { seq: task.seq, outcome }).is_err() {
            break;
        }
        if failed {
            break;
        }
    }
    let _ = batcher.flush(&mut conn);
    Ok(())
}

fn flush_ready_results<T: io::Read + io::Write>(
    transport: &mut T, state: &mut BridgeState, state_file: Option<&Path>, consume: bool, pending: &mut std::collections::VecDeque<PendingExport>,
    result_rx: &mpsc::Receiver<ExportResult>, block: bool,
) -> io::Result<()> {
    loop {
        let Some(front) = pending.front() else {
            return Ok(());
        };

        let result = match front {
            PendingExport::Dropped(seq) => ExportResult { seq: *seq, outcome: Ok(ExportOutcome::DroppedNewest) },
            PendingExport::Queued(expected_seq) => {
                let result = if block {
                    result_rx.recv().map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "collector export worker stopped"))?
                } else {
                    match result_rx.try_recv() {
                        Ok(result) => result,
                        Err(mpsc::TryRecvError::Empty) => return Ok(()),
                        Err(mpsc::TryRecvError::Disconnected) => {
                            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "collector export worker stopped"));
                        }
                    }
                };
                if result.seq != *expected_seq {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("collector export worker returned seq={} out of order; expected {}", result.seq, expected_seq),
                    ));
                }
                result
            }
        };

        pending.pop_front();
        match result.outcome {
            Ok(ExportOutcome::Delivered) => commit_record(transport, state, state_file, consume, result.seq)?,
            Ok(ExportOutcome::DroppedNewest) => {
                commit_record(transport, state, state_file, consume, result.seq)?;
            }
            Err(err) => return Err(err),
        }
    }
}

fn commit_record<T: io::Read + io::Write>(
    transport: &mut T, state: &mut BridgeState, state_file: Option<&Path>, consume: bool, seq: u64,
) -> io::Result<()> {
    state.last_seq = seq;
    write_bridge_state(state_file, state)?;
    if consume {
        write_replay_ack(transport, &ReplayAck { ack_seq: seq })?;
        transport.flush()?;
    }
    Ok(())
}

fn connect_with_timeout(authority: &str, timeout: Duration) -> io::Result<TcpStream> {
    let mut last_err = None;
    for addr in authority.to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(stream) => return Ok(stream),
            Err(err) => last_err = Some(err),
        }
    }

    Err(last_err
        .unwrap_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("collector or upstream address could not be resolved: {authority}"))))
}

fn to_io_error(err: logjet::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BridgeState {
    stream_id: Option<u64>,
    last_seq: u64,
}

fn read_bridge_state(path: Option<&Path>) -> io::Result<BridgeState> {
    let Some(path) = path else {
        return Ok(BridgeState { stream_id: None, last_seq: 0 });
    };
    if !path.exists() {
        return Ok(BridgeState { stream_id: None, last_seq: 0 });
    }

    let text = fs::read_to_string(path)?;
    parse_bridge_state(&text)
}

fn write_bridge_state(path: Option<&Path>, state: &BridgeState) -> io::Result<()> {
    let Some(path) = path else {
        return Ok(());
    };
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        path,
        format!("stream_id={}\nlast_seq={}\n", state.stream_id.map(|value| value.to_string()).unwrap_or_else(|| "unset".to_string()), state.last_seq),
    )
}

fn parse_bridge_state(text: &str) -> io::Result<BridgeState> {
    if let Ok(last_seq) = text.trim().parse::<u64>() {
        return Ok(BridgeState { stream_id: None, last_seq });
    }

    let mut stream_id = None;
    let mut last_seq = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "stream_id" => {
                let value = value.trim();
                stream_id = if value == "unset" {
                    Some(None)
                } else {
                    Some(Some(
                        value
                            .parse::<u64>()
                            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, format!("invalid bridge state stream_id: {err}")))?,
                    ))
                };
            }
            "last_seq" => {
                last_seq = Some(
                    value
                        .trim()
                        .parse::<u64>()
                        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, format!("invalid bridge state last_seq: {err}")))?,
                );
            }
            _ => {}
        }
    }

    let last_seq = last_seq.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid bridge state: missing last_seq"))?;
    Ok(BridgeState { stream_id: stream_id.unwrap_or(None), last_seq })
}

fn reconcile_bridge_state(source: &str, state: &mut BridgeState, hello: &ReplayHello) -> io::Result<()> {
    if let Some(saved_stream_id) = state.stream_id {
        if saved_stream_id != hello.stream_id {
            eprintln!(
                "bridge source {source} changed stream identity {} -> {}; resetting saved seq from {} to 0",
                saved_stream_id, hello.stream_id, state.last_seq
            );
            state.last_seq = 0;
        }
    } else if state.last_seq > 0 && hello.last_seq > 0 && hello.last_seq < state.last_seq {
        eprintln!(
            "bridge source {source} appears to have reset or been replaced; upstream last_seq={} is below saved seq={}; resetting to 0",
            hello.last_seq, state.last_seq
        );
        state.last_seq = 0;
    }
    state.stream_id = Some(hello.stream_id);
    Ok(())
}

#[derive(Clone)]
struct CollectorEndpoint {
    authority: String,
    path: String,
    tls: bool,
    grpc: bool,
}

#[derive(Clone)]
struct CollectorTransport {
    endpoints: Vec<CollectorEndpoint>,
    timeout: Duration,
    backpressure_enabled: bool,
    backpressure_mode: BackpressureMode,
    max_buffered_records: usize,
    collector: CollectorConfig,
    upstream_mode: UpstreamMode,
}

impl CollectorTransport {
    fn describe_backpressure(&self) -> String {
        if !self.backpressure_enabled {
            return "disabled".to_string();
        }
        format!(
            "{} buffered={}",
            match self.backpressure_mode {
                BackpressureMode::Block => "block",
                BackpressureMode::Disconnect => "disconnect",
                BackpressureMode::DropNewest => "drop-newest",
            },
            self.max_buffered_records
        )
    }

    fn open_connection(&self) -> io::Result<MultiCollectorConnection> {
        MultiCollectorConnection::connect(&self.endpoints, self.timeout, &self.collector)
    }
}

enum CollectorStream {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
}

impl io::Read for CollectorStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(s) => s.read(buf),
            Self::Tls(s) => s.read(buf),
        }
    }
}

impl io::Write for CollectorStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Plain(s) => s.write(buf),
            Self::Tls(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Plain(s) => s.flush(),
            Self::Tls(s) => s.flush(),
        }
    }
}

/// Persistent HTTP/1.1 keep-alive connection to an OTLP collector.
enum CollectorConnection {
    Http(Box<HttpCollectorConnection>),
    Grpc(Box<GrpcCollectorConnection>),
}

struct MultiCollectorConnection {
    collectors: Vec<CollectorConnection>,
}

struct HttpCollectorConnection {
    stream: CollectorStream,
    endpoint: CollectorEndpoint,
    timeout: Duration,
    tls_client: Option<Arc<ClientConfig>>,
    collector: CollectorConfig,
}

struct GrpcCollectorConnection {
    client: LogsServiceClient<Channel>,
    endpoint: CollectorEndpoint,
    collector: CollectorConfig,
    timeout: Duration,
    runtime: Runtime,
}

impl CollectorConnection {
    fn connect(
        endpoint: &CollectorEndpoint, timeout: Duration, tls_client: Option<&Arc<ClientConfig>>, collector: &CollectorConfig,
    ) -> io::Result<Self> {
        if endpoint.grpc {
            return GrpcCollectorConnection::connect(endpoint, timeout, collector).map(Box::new).map(Self::Grpc);
        }
        HttpCollectorConnection::connect(endpoint, timeout, tls_client, collector).map(Box::new).map(Self::Http)
    }

    fn reconnect(&mut self) -> io::Result<()> {
        match self {
            Self::Http(conn) => conn.reconnect(),
            Self::Grpc(conn) => conn.reconnect(),
        }
    }

    /// POST one OTLP payload, reconnecting once on transport failure.
    fn post(&mut self, payload: &[u8]) -> io::Result<()> {
        match self.post_inner(payload) {
            Ok(()) => Ok(()),
            Err(_first) => {
                self.reconnect()?;
                self.post_inner(payload)
            }
        }
    }

    fn post_inner(&mut self, payload: &[u8]) -> io::Result<()> {
        match self {
            Self::Http(conn) => conn.post_inner(payload),
            Self::Grpc(conn) => conn.post_inner(payload),
        }
    }
}

impl MultiCollectorConnection {
    fn connect(endpoints: &[CollectorEndpoint], timeout: Duration, collector: &CollectorConfig) -> io::Result<Self> {
        let mut collectors = Vec::with_capacity(endpoints.len());
        for endpoint in endpoints {
            let tls_client = if endpoint.tls && !endpoint.grpc { Some(load_collector_client_config(collector)?) } else { None };
            collectors.push(CollectorConnection::connect(endpoint, timeout, tls_client.as_ref(), collector)?);
        }
        Ok(Self { collectors })
    }

    fn post(&mut self, payload: &[u8]) -> io::Result<()> {
        for collector in &mut self.collectors {
            collector.post(payload)?;
        }
        Ok(())
    }
}

impl HttpCollectorConnection {
    fn connect(
        endpoint: &CollectorEndpoint, timeout: Duration, tls_client: Option<&Arc<ClientConfig>>, collector: &CollectorConfig,
    ) -> io::Result<Self> {
        let tcp = connect_with_timeout(&endpoint.authority, timeout)?;
        tcp.set_write_timeout(Some(timeout))?;
        tcp.set_read_timeout(Some(timeout))?;
        let stream = if let Some(cfg) = tls_client {
            let server_name = parse_collector_server_name(collector, &endpoint.authority)?;
            let conn = ClientConnection::new(cfg.clone(), server_name).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
            CollectorStream::Tls(Box::new(StreamOwned::new(conn, tcp)))
        } else {
            CollectorStream::Plain(tcp)
        };
        Ok(Self { stream, endpoint: endpoint.clone(), timeout, tls_client: tls_client.cloned(), collector: collector.clone() })
    }

    fn reconnect(&mut self) -> io::Result<()> {
        *self = Self::connect(&self.endpoint, self.timeout, self.tls_client.as_ref(), &self.collector)?;
        Ok(())
    }

    fn post_inner(&mut self, payload: &[u8]) -> io::Result<()> {
        write!(
            self.stream,
            "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/x-protobuf\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
            self.endpoint.path,
            self.endpoint.authority,
            payload.len()
        )?;
        self.stream.write_all(payload)?;
        self.stream.flush()?;
        read_http_response(&mut self.stream)
    }
}

impl GrpcCollectorConnection {
    fn connect(endpoint: &CollectorEndpoint, timeout: Duration, collector: &CollectorConfig) -> io::Result<Self> {
        let runtime =
            Builder::new_current_thread().enable_all().build().map_err(|err| io::Error::other(format!("failed to build gRPC runtime: {err}")))?;
        let client = runtime.block_on(connect_grpc_with_collector(endpoint, timeout, Some(collector)))?;
        Ok(Self { client, endpoint: endpoint.clone(), collector: collector.clone(), timeout, runtime })
    }

    fn reconnect(&mut self) -> io::Result<()> {
        self.client = self.runtime.block_on(connect_grpc_with_collector(&self.endpoint, self.timeout, Some(&self.collector)))?;
        Ok(())
    }

    fn post_inner(&mut self, payload: &[u8]) -> io::Result<()> {
        let req = ExportLogsServiceRequest::decode(payload)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, format!("invalid OTLP log batch: {err}")))?;
        self.runtime
            .block_on(self.client.export(Request::new(req)))
            .map(|_| ())
            .map_err(|err| io::Error::other(format!("collector gRPC export failed: {err}")))
    }
}

/// Read an HTTP/1.1 response, consuming exactly the headers + body.
fn read_http_response(stream: &mut impl io::Read) -> io::Result<()> {
    let mut hdr_buf = Vec::with_capacity(512);
    let mut b = [0u8; 1];
    loop {
        if stream.read(&mut b)? == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "collector closed connection before response headers"));
        }
        hdr_buf.push(b[0]);
        if hdr_buf.len() >= 4 && &hdr_buf[hdr_buf.len() - 4..] == b"\r\n\r\n" {
            break;
        }
        if hdr_buf.len() > 16_384 {
            return Err(io::Error::other("collector response headers exceed 16 KiB"));
        }
    }

    let hdr_str = String::from_utf8_lossy(&hdr_buf);
    let status_line = hdr_str.lines().next().unwrap_or("");
    if !status_line.starts_with("HTTP/1.1 200") && !status_line.starts_with("HTTP/1.0 200") {
        return Err(io::Error::other(format!("collector returned non-200 response: {status_line}")));
    }

    let content_len = parse_content_length(&hdr_str);
    if content_len > 0 {
        let mut remaining = content_len;
        let mut discard = [0u8; 4096];
        while remaining > 0 {
            let chunk = discard.len().min(remaining);
            let n = stream.read(&mut discard[..chunk])?;
            if n == 0 {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "collector closed connection during response body"));
            }
            remaining -= n;
        }
    }

    Ok(())
}

fn parse_content_length(headers: &str) -> usize {
    for line in headers.lines() {
        if let Some(val) = line.strip_prefix("Content-Length:").or_else(|| line.strip_prefix("content-length:"))
            && let Ok(n) = val.trim().parse::<usize>()
        {
            return n;
        }
    }
    0
}

/// Accumulates OTLP payloads and merges them into combined ExportLogsServiceRequests
/// grouped by Resource+Scope before posting to the collector.
struct OtlpBatcher {
    batch_size: usize,
    batch_timeout: Duration,
    pending: ExportLogsServiceRequest,
    pending_count: usize,
    first_added: Option<Instant>,
}

impl OtlpBatcher {
    fn new(batch_size: usize, batch_timeout_ms: u64) -> Self {
        Self {
            batch_size,
            batch_timeout: Duration::from_millis(batch_timeout_ms),
            pending: ExportLogsServiceRequest { resource_logs: Vec::new() },
            pending_count: 0,
            first_added: None,
        }
    }

    /// Add a raw OTLP payload to the batch. Flushes to conn if batch is full.
    fn add(&mut self, payload: &[u8], conn: &mut MultiCollectorConnection) -> io::Result<()> {
        if self.batch_size <= 1 {
            return conn.post(payload);
        }
        match ExportLogsServiceRequest::decode(payload) {
            Ok(req) => self.merge(req),
            Err(_) => return conn.post(payload),
        }
        if self.pending_count >= self.batch_size {
            self.flush(conn)?;
        }
        Ok(())
    }

    /// Flush any pending records if the batch timeout has expired.
    fn flush_if_expired(&mut self, conn: &mut MultiCollectorConnection) -> io::Result<()> {
        if self.pending_count > 0 && self.batch_timeout.as_millis() > 0 && self.first_added.is_some_and(|t| t.elapsed() >= self.batch_timeout) {
            self.flush(conn)?;
        }
        Ok(())
    }

    /// Flush all pending records to the collector.
    fn flush(&mut self, conn: &mut MultiCollectorConnection) -> io::Result<()> {
        if self.pending_count == 0 {
            return Ok(());
        }
        let merged = std::mem::replace(&mut self.pending, ExportLogsServiceRequest { resource_logs: Vec::new() });
        self.pending_count = 0;
        self.first_added = None;
        conn.post(&merged.encode_to_vec())
    }

    fn merge(&mut self, req: ExportLogsServiceRequest) {
        if self.first_added.is_none() {
            self.first_added = Some(Instant::now());
        }
        for incoming_rl in req.resource_logs {
            let existing = self.pending.resource_logs.iter_mut().find(|rl| rl.resource == incoming_rl.resource);
            match existing {
                Some(rl) => {
                    for incoming_sl in incoming_rl.scope_logs {
                        let existing_sl = rl.scope_logs.iter_mut().find(|sl| sl.scope == incoming_sl.scope);
                        match existing_sl {
                            Some(sl) => {
                                self.pending_count += incoming_sl.log_records.len();
                                sl.log_records.extend(incoming_sl.log_records);
                            }
                            None => {
                                self.pending_count += incoming_sl.log_records.len();
                                rl.scope_logs.push(incoming_sl);
                            }
                        }
                    }
                }
                None => {
                    for sl in &incoming_rl.scope_logs {
                        self.pending_count += sl.log_records.len();
                    }
                    self.pending.resource_logs.push(incoming_rl);
                }
            }
        }
    }
}

#[derive(Debug)]
struct ExportTask {
    seq: u64,
    payload: Vec<u8>,
}

struct ExportResult {
    seq: u64,
    outcome: io::Result<ExportOutcome>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExportOutcome {
    Delivered,
    DroppedNewest,
}

#[derive(Debug)]
enum PendingExport {
    Queued(u64),
    Dropped(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EnqueueOutcome {
    Queued,
    DroppedNewest,
}

impl CollectorEndpoint {
    fn parse(input: &str) -> io::Result<Self> {
        if let Some(authority) = input.strip_prefix("grpc://") {
            if authority.is_empty() {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "collector.url missing host:port"));
            }
            return Ok(Self { authority: authority.to_string(), path: String::new(), tls: false, grpc: true });
        }

        if let Some(authority) = input.strip_prefix("grpcs://") {
            if authority.is_empty() {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "collector.url missing host:port"));
            }
            return Ok(Self { authority: authority.to_string(), path: String::new(), tls: true, grpc: true });
        }

        if let Some(rest) = input.strip_prefix("http://") {
            let (authority, path) = split_authority_and_path(rest);
            if authority.is_empty() {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "collector.url missing host:port"));
            }
            return Ok(Self { authority: authority.to_string(), path: normalise_path(path), tls: false, grpc: false });
        }

        if let Some(rest) = input.strip_prefix("https://") {
            let (authority, path) = split_authority_and_path(rest);
            if authority.is_empty() {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "collector.url missing host:port"));
            }
            return Ok(Self { authority: authority.to_string(), path: normalise_path(path), tls: true, grpc: false });
        }

        Ok(Self { authority: input.to_string(), path: "/v1/logs".to_string(), tls: false, grpc: false })
    }
}

async fn connect_grpc_with_collector(
    endpoint: &CollectorEndpoint, timeout: Duration, collector: Option<&CollectorConfig>,
) -> io::Result<LogsServiceClient<Channel>> {
    let mut client = Endpoint::from_shared(format!("{}://{}", if endpoint.tls { "https" } else { "http" }, endpoint.authority))
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))?
        .timeout(timeout)
        .connect_timeout(timeout);
    if endpoint.tls {
        let collector = collector.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing collector config for grpcs:// export"))?;
        let ca_file = collector
            .ca_file
            .as_deref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "collector.ca-file is required for grpcs:// collector.url"))?;
        let domain = collector.server_name.as_deref().unwrap_or_else(|| authority_host(&endpoint.authority));
        let tls = match (collector.cert_file.as_deref(), collector.key_file.as_deref()) {
            (Some(cert_file), Some(key_file)) => ClientTlsConfig::new()
                .ca_certificate(Certificate::from_pem(fs::read(ca_file)?))
                .identity(Identity::from_pem(fs::read(cert_file)?, fs::read(key_file)?))
                .domain_name(domain.to_string()),
            (None, None) => ClientTlsConfig::new().ca_certificate(Certificate::from_pem(fs::read(ca_file)?)).domain_name(domain.to_string()),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "collector.cert-file and collector.key-file must either both be set or both be unset",
                ));
            }
        };
        client = client.tls_config(tls).map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))?;
    }
    client.connect().await.map(LogsServiceClient::new).map_err(|err| io::Error::other(format!("failed to connect gRPC collector: {err}")))
}

fn split_authority_and_path(input: &str) -> (&str, &str) {
    match input.find('/') {
        Some(index) => (&input[..index], &input[index..]),
        None => (input, "/v1/logs"),
    }
}

fn normalise_path(path: &str) -> String {
    if path.is_empty() {
        "/v1/logs".to_string()
    } else if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

fn parse_collector_endpoints(collector: &CollectorConfig) -> io::Result<Vec<CollectorEndpoint>> {
    collector.urls.iter().map(|url| CollectorEndpoint::parse(url)).collect()
}

#[cfg(test)]
#[path = "../tests/unit/replay_utst.rs"]
mod replay_utst;
