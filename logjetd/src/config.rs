use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct Config {
    pub ingest_addr: String,
    pub ingest_protocol: IngestProtocol,
    pub replay_addr: String,
    pub poll_interval_ms: u64,
    pub storage: StorageConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestProtocol {
    Wire,
    OtlpHttp,
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
    #[serde(rename = "ingest.listen")]
    ingest_addr: Option<String>,
    #[serde(rename = "ingest.protocol")]
    ingest_protocol: Option<String>,
    #[serde(rename = "replay.listen")]
    replay_addr: Option<String>,
    #[serde(rename = "replay.poll_ms")]
    poll_interval_ms: Option<u64>,
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
                ingest_addr: None,
                ingest_protocol: None,
                replay_addr: None,
                poll_interval_ms: None,
            }
        } else {
            return Err(format!("config file not found: {}", path.display()).into());
        };

        let output = raw.output.unwrap_or_else(|| "buffer".to_string());
        let ingest_addr = raw
            .ingest_addr
            .unwrap_or_else(|| "127.0.0.1:7001".to_string());
        let ingest_protocol = match raw.ingest_protocol.as_deref().unwrap_or("wire") {
            "wire" => IngestProtocol::Wire,
            "otlp-http" => IngestProtocol::OtlpHttp,
            other => return Err(format!("invalid ingest protocol: {other}").into()),
        };
        let replay_addr = raw
            .replay_addr
            .unwrap_or_else(|| "0.0.0.0:7002".to_string());
        let poll_interval_ms = raw.poll_interval_ms.unwrap_or(250);
        let keep_messages = raw.buffer_keep.unwrap_or(0);

        let storage = match output.as_str() {
            "buffer" => StorageConfig::Buffer(BufferConfig {
                limit: parse_buffer_limit(raw.buffer_size_kb, raw.buffer_messages)?,
                keep_messages,
            }),
            "file" => {
                let name = raw
                    .file_name
                    .unwrap_or_else(|| "bar.logjet".to_string());
                StorageConfig::File(FileConfig {
                    dir: raw.file_path.unwrap_or_else(|| PathBuf::from(".")),
                    name,
                    segment_size_bytes: u64::try_from(kib_to_bytes(raw.file_size_kb.unwrap_or(100))?)?,
                })
            }
            other => return Err(format!("invalid output mode: {other}").into()),
        };

        Ok(Self {
            ingest_addr,
            ingest_protocol,
            replay_addr,
            poll_interval_ms,
            storage,
        })
    }
}

fn kib_to_bytes(value: u64) -> Result<usize, Box<dyn std::error::Error>> {
    let bytes = value
        .checked_mul(1024)
        .ok_or("size overflow while converting KiB to bytes")?;
    Ok(usize::try_from(bytes)?)
}

fn parse_buffer_limit(
    size_kib: Option<u64>,
    messages: Option<usize>,
) -> Result<BufferLimit, Box<dyn std::error::Error>> {
    match (size_kib, messages) {
        (Some(_), Some(_)) => Err("buffer.size and buffer.messages conflict; set only one".into()),
        (Some(size_kib), None) => Ok(BufferLimit::Bytes(kib_to_bytes(size_kib)?)),
        (None, Some(messages)) => Ok(BufferLimit::Messages(messages)),
        (None, None) => Ok(BufferLimit::Bytes(kib_to_bytes(100)?)),
    }
}
