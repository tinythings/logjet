use std::io::{self, BufReader, BufWriter, Write};
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
use tonic::{Request, Response as GrpcResponse, Status};

use crate::config::{Config, IngestProtocol};
use crate::protocol::read_record;
use crate::spool::Spool;
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

    let replay_thread = thread::Builder::new()
        .name("logjetd-replay".to_string())
        .spawn(move || replay_loop(replay_addr, replay_spool, poll_interval_ms))?;

    eprintln!("logjetd using config {}", config.config_path.display());
    ingest_loop(
        config.config.ingest_addr,
        config.config.ingest_protocol,
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
    spool: Arc<Mutex<Spool>>,
    next_seq: Arc<AtomicU64>,
) -> io::Result<()> {
    match protocol {
        IngestProtocol::Wire => {
            let listener = TcpListener::bind(&bind_addr)?;
            eprintln!("logjetd ingest listening on {bind_addr} using wire protocol");

            for stream in listener.incoming() {
                let stream = stream?;
                let spool = Arc::clone(&spool);
                thread::Builder::new()
                    .name("logjetd-ingest-client".to_string())
                    .spawn(move || {
                        if let Err(err) = handle_ingest_client(stream, spool) {
                            eprintln!("logjetd ingest client error: {err}");
                        }
                    })?;
            }
        }
        IngestProtocol::OtlpHttp => {
            let server = Server::http(&bind_addr)
                .map_err(|err| io::Error::other(err.to_string()))?;
            eprintln!("logjetd ingest listening on http://{bind_addr}/v1/logs using otlp-http");

            for mut request in server.incoming_requests() {
                if request.method() != &Method::Post || request.url() != "/v1/logs" {
                    let response = Response::from_string("not found")
                        .with_status_code(StatusCode(404));
                    let _ = request.respond(response);
                    continue;
                }

                let mut body = Vec::new();
                request.as_reader().read_to_end(&mut body)?;
                match ExportLogsServiceRequest::decode(body.as_slice()) {
                    Ok(batch) => {
                        let record = WireRecord {
                            record_type: logjet::RecordType::Logs,
                            seq: next_seq.fetch_add(1, Ordering::Relaxed),
                            ts_unix_ns: extract_batch_timestamp(&batch).unwrap_or_else(unix_time_nanos),
                            payload: body,
                        };

                        let mut spool = spool
                            .lock()
                            .map_err(|_| io::Error::other("spool mutex poisoned"))?;
                        spool.append(record)?;

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
                "logjetd ingest listening on grpc://{bind_addr} using otlp-grpc"
            );

            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|err| io::Error::other(err.to_string()))?;
            let service = OtlpGrpcLogsService { spool, next_seq };

            runtime.block_on(async move {
                tonic::transport::Server::builder()
                    .add_service(LogsServiceServer::new(service))
                    .serve(addr)
                    .await
                    .map_err(|err| io::Error::other(err.to_string()))
            })?;
        }
    }

    Ok(())
}

fn replay_loop(bind_addr: String, spool: Arc<Mutex<Spool>>, poll_interval_ms: u64) -> io::Result<()> {
    let listener = TcpListener::bind(&bind_addr)?;
    eprintln!("logjetd replay listening on {bind_addr}");

    for stream in listener.incoming() {
        let stream = stream?;
        let spool = Arc::clone(&spool);
        thread::Builder::new()
            .name("logjetd-replay-client".to_string())
            .spawn(move || {
                if let Err(err) = handle_replay_client(stream, spool, poll_interval_ms) {
                    eprintln!("logjetd replay client error: {err}");
                }
            })?;
    }

    Ok(())
}

fn handle_ingest_client(stream: TcpStream, spool: Arc<Mutex<Spool>>) -> io::Result<()> {
    let peer = stream.peer_addr().ok();
    let mut reader = BufReader::new(stream);

    while let Some(record) = read_record(&mut reader)? {
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

fn handle_replay_client(
    stream: TcpStream,
    spool: Arc<Mutex<Spool>>,
    poll_interval_ms: u64,
) -> io::Result<()> {
    let mut writer = BufWriter::new(stream);
    let mut last_seq = 0u64;
    let sleep = Duration::from_millis(poll_interval_ms);

    loop {
        let sent_any = {
            let spool = spool
                .lock()
                .map_err(|_| io::Error::other("spool mutex poisoned"))?;
            spool.replay_since(&mut writer, &mut last_seq)?
        };

        if sent_any {
            writer.flush()?;
        } else {
            writer.flush()?;
            thread::sleep(sleep);
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
