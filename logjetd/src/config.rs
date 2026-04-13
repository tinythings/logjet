use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct Config {
    pub ingest_addr: String,
    pub ingest_protocol: IngestProtocol,
    pub ingest_tls: IngestTlsConfig,
    pub ingest_limits: IngestLimits,
    pub ingest_overload: IngestOverloadConfig,
    pub replay_addr: String,
    pub replay_max_clients: usize,
    pub replay_client_timeout_ms: u64,
    /// LZ4 compress wire protocol payloads. Disable on low-power CPUs.
    pub wire_compression: bool,
    pub collector: CollectorConfig,
    pub backpressure: BackpressureConfig,
    pub upstream: UpstreamConfig,
    pub tls: TlsConfig,
    pub storage: StorageConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestProtocol {
    Wire,
    OtlpHttp,
    OtlpGrpc,
}

#[derive(Debug, Clone)]
pub enum StorageConfig {
    Buffer(BufferConfig),
    File(FileConfig),
}

#[derive(Debug, Clone)]
pub struct BufferConfig {
    pub limit: BufferLimit,
    pub keep_messages: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferLimit {
    Bytes(usize),
    Messages(usize),
}

#[derive(Debug, Clone)]
pub struct FileConfig {
    pub dir: PathBuf,
    pub name: String,
    pub segment_size_bytes: u64,
    pub fsync: FsyncPolicy,
    /// Block compression codec for .logjet files.
    pub codec: logjet::Codec,
    /// Maximum total bytes across all segments. 0 = unlimited.
    pub max_total_bytes: u64,
    /// Pad each block to this alignment (bytes). 0 = no padding.
    pub block_alignment: usize,
}

/// Controls when data is guaranteed durable on disk via fsync().
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsyncPolicy {
    /// Never fsync — fastest, data may be lost on power cut.
    None,
    /// Fsync after every block flush — safest, slowest.
    Block,
    /// Fsync periodically in the background flush thread.
    Interval,
}

#[derive(Debug, Clone)]
pub struct CollectorConfig {
    pub url: String,
    pub timeout_ms: u64,
    pub ca_file: Option<PathBuf>,
    pub cert_file: Option<PathBuf>,
    pub key_file: Option<PathBuf>,
    pub server_name: Option<String>,
    /// Merge up to this many stored OTLP requests into one POST. 1 = no re-batching.
    pub batch_size: usize,
    /// Flush a partial batch after this many ms. 0 = flush only on batch_size.
    pub batch_timeout_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackpressureConfig {
    pub enabled: bool,
    pub mode: BackpressureMode,
    pub max_buffered_records: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackpressureMode {
    Block,
    Disconnect,
    DropNewest,
}

#[derive(Debug, Clone)]
pub struct UpstreamConfig {
    pub replay_addr: Option<String>,
    pub mode: UpstreamMode,
    pub state_file: Option<PathBuf>,
    pub retry_ms: u64,
    pub connect_timeout_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamMode {
    Keep,
    Drain,
}

#[derive(Debug, Clone)]
pub struct TlsConfig {
    pub enable: bool,
    pub ca_file: Option<PathBuf>,
    pub cert_file: Option<PathBuf>,
    pub key_file: Option<PathBuf>,
    pub require_client_cert: bool,
    pub server_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct IngestTlsConfig {
    pub enable: bool,
    pub ca_file: Option<PathBuf>,
    pub cert_file: Option<PathBuf>,
    pub key_file: Option<PathBuf>,
    pub require_client_cert: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IngestLimits {
    pub max_batch_bytes: usize,
    pub max_clients: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IngestOverloadConfig {
    pub max_batches_per_second: u64,
    pub priority_severity_floor: SeverityFloor,
    pub report_every_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeverityFloor {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Fatal,
}

#[derive(Debug, Deserialize)]
struct RawConfig {
    output: Option<String>,
    #[serde(rename = "buffer.size")]
    buffer_size_kb: Option<u64>,
    #[serde(rename = "buffer.messages")]
    buffer_messages: Option<usize>,
    #[serde(rename = "buffer.keep")]
    buffer_keep: Option<usize>,
    #[serde(rename = "file.path")]
    file_path: Option<PathBuf>,
    #[serde(rename = "file.size")]
    file_size_kb: Option<u64>,
    #[serde(rename = "file.name")]
    file_name: Option<String>,
    #[serde(rename = "file.fsync")]
    file_fsync: Option<String>,
    #[serde(rename = "file.max-bytes")]
    file_max_bytes_kb: Option<u64>,
    #[serde(rename = "file.codec")]
    file_codec: Option<String>,
    #[serde(rename = "file.block-alignment")]
    file_block_alignment: Option<usize>,
    #[serde(rename = "ingest.listen")]
    ingest_addr: Option<String>,
    #[serde(rename = "ingest.protocol")]
    ingest_protocol: Option<String>,
    #[serde(rename = "ingest.tls-enable")]
    ingest_tls_enable: Option<bool>,
    #[serde(rename = "ingest.ca-file")]
    ingest_ca_file: Option<PathBuf>,
    #[serde(rename = "ingest.cert-file")]
    ingest_cert_file: Option<PathBuf>,
    #[serde(rename = "ingest.key-file")]
    ingest_key_file: Option<PathBuf>,
    #[serde(rename = "ingest.require-client-cert")]
    ingest_require_client_cert: Option<bool>,
    #[serde(rename = "ingest.max-batch-bytes")]
    ingest_max_batch_bytes: Option<usize>,
    #[serde(rename = "ingest.max-clients")]
    ingest_max_clients: Option<usize>,
    #[serde(rename = "ingest.max-batches-per-second")]
    ingest_max_batches_per_second: Option<u64>,
    #[serde(rename = "ingest.priority-severity-at-least")]
    ingest_priority_severity_floor: Option<String>,
    #[serde(rename = "ingest.overload-report-ms")]
    ingest_overload_report_ms: Option<u64>,
    #[serde(rename = "replay.listen")]
    replay_addr: Option<String>,
    #[serde(rename = "replay.max-clients")]
    replay_max_clients: Option<usize>,
    #[serde(rename = "replay.client-timeout-ms")]
    replay_client_timeout_ms: Option<u64>,
    #[serde(rename = "wire.compression")]
    wire_compression: Option<bool>,
    #[serde(rename = "collector.url")]
    collector_url: Option<String>,
    #[serde(rename = "collector.timeout-ms")]
    collector_timeout_ms: Option<u64>,
    #[serde(rename = "collector.ca-file")]
    collector_ca_file: Option<PathBuf>,
    #[serde(rename = "collector.cert-file")]
    collector_cert_file: Option<PathBuf>,
    #[serde(rename = "collector.key-file")]
    collector_key_file: Option<PathBuf>,
    #[serde(rename = "collector.server-name")]
    collector_server_name: Option<String>,
    #[serde(rename = "collector.batch-size")]
    collector_batch_size: Option<usize>,
    #[serde(rename = "collector.batch-timeout-ms")]
    collector_batch_timeout_ms: Option<u64>,
    #[serde(rename = "backpressure.enabled")]
    backpressure_enabled: Option<bool>,
    #[serde(rename = "backpressure.mode")]
    backpressure_mode: Option<String>,
    #[serde(rename = "backpressure.max-buffered-records")]
    backpressure_max_buffered_records: Option<usize>,
    #[serde(rename = "upstream.replay")]
    upstream_replay_addr: Option<String>,
    #[serde(rename = "upstream.mode")]
    upstream_mode: Option<String>,
    #[serde(rename = "upstream.state-file")]
    upstream_state_file: Option<PathBuf>,
    #[serde(rename = "upstream.retry-ms")]
    upstream_retry_ms: Option<u64>,
    #[serde(rename = "upstream.connect-timeout-ms")]
    upstream_connect_timeout_ms: Option<u64>,
    #[serde(rename = "tls.enable")]
    tls_enable: Option<bool>,
    #[serde(rename = "tls.ca-file")]
    tls_ca_file: Option<PathBuf>,
    #[serde(rename = "tls.cert-file")]
    tls_cert_file: Option<PathBuf>,
    #[serde(rename = "tls.key-file")]
    tls_key_file: Option<PathBuf>,
    #[serde(rename = "tls.require-client-cert")]
    tls_require_client_cert: Option<bool>,
    #[serde(rename = "tls.server-name")]
    tls_server_name: Option<String>,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let raw = if path.exists() {
            serde_yaml::from_str::<RawConfig>(&fs::read_to_string(path)?)?
        } else if path == Path::new("/etc/logjet.conf") {
            RawConfig {
                output: None,
                buffer_size_kb: None,
                buffer_messages: None,
                buffer_keep: None,
                file_path: None,
                file_size_kb: None,
                file_name: None,
                file_fsync: None,
                file_max_bytes_kb: None,
                file_codec: None,
                file_block_alignment: None,
                ingest_addr: None,
                ingest_protocol: None,
                ingest_tls_enable: None,
                ingest_ca_file: None,
                ingest_cert_file: None,
                ingest_key_file: None,
                ingest_require_client_cert: None,
                ingest_max_batch_bytes: None,
                ingest_max_clients: None,
                ingest_max_batches_per_second: None,
                ingest_priority_severity_floor: None,
                ingest_overload_report_ms: None,
                replay_addr: None,
                replay_max_clients: None,
                replay_client_timeout_ms: None,
                wire_compression: None,
                collector_url: None,
                collector_timeout_ms: None,
                collector_ca_file: None,
                collector_cert_file: None,
                collector_key_file: None,
                collector_server_name: None,
                collector_batch_size: None,
                collector_batch_timeout_ms: None,
                backpressure_enabled: None,
                backpressure_mode: None,
                backpressure_max_buffered_records: None,
                upstream_replay_addr: None,
                upstream_mode: None,
                upstream_state_file: None,
                upstream_retry_ms: None,
                upstream_connect_timeout_ms: None,
                tls_enable: None,
                tls_ca_file: None,
                tls_cert_file: None,
                tls_key_file: None,
                tls_require_client_cert: None,
                tls_server_name: None,
            }
        } else {
            return Err(format!("config file not found: {}", path.display()).into());
        };

        let output = raw.output.unwrap_or_else(|| "buffer".to_string());
        let ingest_addr = raw.ingest_addr.unwrap_or_else(|| "127.0.0.1:7001".to_string());
        let ingest_protocol = match raw.ingest_protocol.as_deref().unwrap_or("wire") {
            "wire" => IngestProtocol::Wire,
            "otlp-http" => IngestProtocol::OtlpHttp,
            "otlp-grpc" => IngestProtocol::OtlpGrpc,
            other => return Err(format!("invalid ingest protocol: {other}").into()),
        };
        let ingest_tls = IngestTlsConfig {
            enable: raw.ingest_tls_enable.unwrap_or(false),
            ca_file: raw.ingest_ca_file,
            cert_file: raw.ingest_cert_file,
            key_file: raw.ingest_key_file,
            require_client_cert: raw.ingest_require_client_cert.unwrap_or(false),
        };
        let ingest_limits =
            IngestLimits { max_batch_bytes: raw.ingest_max_batch_bytes.unwrap_or(1024 * 1024), max_clients: raw.ingest_max_clients.unwrap_or(32) };
        let ingest_overload = IngestOverloadConfig {
            max_batches_per_second: raw.ingest_max_batches_per_second.unwrap_or(0),
            priority_severity_floor: parse_severity_floor(raw.ingest_priority_severity_floor.as_deref().unwrap_or("error"))?,
            report_every_ms: raw.ingest_overload_report_ms.unwrap_or(5_000),
        };
        if ingest_limits.max_batch_bytes == 0 {
            return Err("ingest.max-batch-bytes must be greater than zero".into());
        }
        if ingest_limits.max_clients == 0 {
            return Err("ingest.max-clients must be greater than zero".into());
        }
        let replay_addr = raw.replay_addr.unwrap_or_else(|| "0.0.0.0:7002".to_string());
        let replay_max_clients = raw.replay_max_clients.unwrap_or(32);
        if replay_max_clients == 0 {
            return Err("replay.max-clients must be greater than zero".into());
        }
        let replay_client_timeout_ms = raw.replay_client_timeout_ms.unwrap_or(10_000);
        if replay_client_timeout_ms == 0 {
            return Err("replay.client-timeout-ms must be greater than zero".into());
        }
        let collector = CollectorConfig {
            url: raw.collector_url.unwrap_or_else(|| "http://127.0.0.1:4318/v1/logs".to_string()),
            timeout_ms: raw.collector_timeout_ms.unwrap_or(10_000),
            ca_file: raw.collector_ca_file,
            cert_file: raw.collector_cert_file,
            key_file: raw.collector_key_file,
            server_name: raw.collector_server_name,
            batch_size: raw.collector_batch_size.unwrap_or(50).max(1),
            batch_timeout_ms: raw.collector_batch_timeout_ms.unwrap_or(200),
        };
        let backpressure = BackpressureConfig {
            enabled: raw.backpressure_enabled.unwrap_or(false),
            mode: match raw.backpressure_mode.as_deref().unwrap_or("disconnect") {
                "block" => BackpressureMode::Block,
                "disconnect" => BackpressureMode::Disconnect,
                "drop-newest" => BackpressureMode::DropNewest,
                other => return Err(format!("invalid backpressure mode: {other}").into()),
            },
            max_buffered_records: raw.backpressure_max_buffered_records.unwrap_or(16),
        };
        if backpressure.max_buffered_records == 0 {
            return Err("backpressure.max-buffered-records must be greater than zero".into());
        }
        let upstream = UpstreamConfig {
            replay_addr: raw.upstream_replay_addr,
            mode: match raw.upstream_mode.as_deref().unwrap_or("keep") {
                "keep" => UpstreamMode::Keep,
                "drain" => UpstreamMode::Drain,
                other => return Err(format!("invalid upstream mode: {other}").into()),
            },
            state_file: raw.upstream_state_file,
            retry_ms: raw.upstream_retry_ms.unwrap_or(1_000),
            connect_timeout_ms: raw.upstream_connect_timeout_ms.unwrap_or(5_000),
        };
        let tls = TlsConfig {
            enable: raw.tls_enable.unwrap_or(false),
            ca_file: raw.tls_ca_file,
            cert_file: raw.tls_cert_file,
            key_file: raw.tls_key_file,
            require_client_cert: raw.tls_require_client_cert.unwrap_or(false),
            server_name: raw.tls_server_name,
        };
        let keep_messages = raw.buffer_keep.unwrap_or(0);

        let storage = match output.as_str() {
            "buffer" => StorageConfig::Buffer(BufferConfig { limit: parse_buffer_limit(raw.buffer_size_kb, raw.buffer_messages)?, keep_messages }),
            "file" => {
                let name = raw.file_name.unwrap_or_else(|| "bar.logjet".to_string());
                let fsync = match raw.file_fsync.as_deref().unwrap_or("interval") {
                    "none" => FsyncPolicy::None,
                    "block" => FsyncPolicy::Block,
                    "interval" => FsyncPolicy::Interval,
                    other => return Err(format!("invalid file.fsync policy: {other}").into()),
                };
                let codec = match raw.file_codec.as_deref().unwrap_or("lz4") {
                    "none" => logjet::Codec::None,
                    "lz4" => logjet::Codec::Lz4,
                    "zstd" => logjet::Codec::Zstd,
                    other => return Err(format!("invalid file.codec: {other}").into()),
                };
                StorageConfig::File(FileConfig {
                    dir: raw.file_path.unwrap_or_else(|| PathBuf::from(".")),
                    name,
                    segment_size_bytes: u64::try_from(kib_to_bytes(raw.file_size_kb.unwrap_or(100))?)?,
                    fsync,
                    codec,
                    max_total_bytes: raw.file_max_bytes_kb.map(|kb| kb.saturating_mul(1024)).unwrap_or(0),
                    block_alignment: raw.file_block_alignment.unwrap_or(4096),
                })
            }
            other => return Err(format!("invalid output mode: {other}").into()),
        };

        Ok(Self {
            ingest_addr,
            ingest_protocol,
            ingest_tls,
            ingest_limits,
            ingest_overload,
            replay_addr,
            replay_max_clients,
            replay_client_timeout_ms,
            wire_compression: raw.wire_compression.unwrap_or(true),
            collector,
            backpressure,
            upstream,
            tls,
            storage,
        })
    }
}

fn parse_severity_floor(value: &str) -> Result<SeverityFloor, Box<dyn std::error::Error>> {
    match value {
        "trace" => Ok(SeverityFloor::Trace),
        "debug" => Ok(SeverityFloor::Debug),
        "info" => Ok(SeverityFloor::Info),
        "warn" => Ok(SeverityFloor::Warn),
        "error" => Ok(SeverityFloor::Error),
        "fatal" => Ok(SeverityFloor::Fatal),
        other => Err(format!("invalid ingest.priority-severity-at-least: {other}").into()),
    }
}

fn kib_to_bytes(value: u64) -> Result<usize, Box<dyn std::error::Error>> {
    let bytes = value.checked_mul(1024).ok_or("size overflow while converting KiB to bytes")?;
    Ok(usize::try_from(bytes)?)
}

fn parse_buffer_limit(size_kib: Option<u64>, messages: Option<usize>) -> Result<BufferLimit, Box<dyn std::error::Error>> {
    match (size_kib, messages) {
        (Some(_), Some(_)) => Err("buffer.size and buffer.messages conflict; set only one".into()),
        (Some(size_kib), None) => Ok(BufferLimit::Bytes(kib_to_bytes(size_kib)?)),
        (None, Some(messages)) => Ok(BufferLimit::Messages(messages)),
        (None, None) => Ok(BufferLimit::Bytes(kib_to_bytes(100)?)),
    }
}

#[cfg(test)]
#[path = "config_utst.rs"]
mod config_utst;
