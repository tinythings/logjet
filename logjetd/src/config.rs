use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct Config {
    pub ingest_addr: String,
    pub replay_addr: String,
    pub poll_interval_ms: u64,
    pub storage: StorageConfig,
}

#[derive(Debug, Clone)]
pub enum StorageConfig {
    Buffer(BufferConfig),
    File(FileConfig),
}

#[derive(Debug, Clone)]
pub struct BufferConfig {
    pub size_bytes: usize,
    pub preserve_messages: usize,
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
    #[serde(rename = "buffer.preserve")]
    buffer_preserve: Option<usize>,
    #[serde(rename = "file.path")]
    file_path: Option<PathBuf>,
    #[serde(rename = "file.size")]
    file_size_kb: Option<u64>,
    #[serde(rename = "file.name")]
    file_name: Option<String>,
    #[serde(rename = "ingest.listen")]
    ingest_addr: Option<String>,
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
                buffer_preserve: None,
                file_path: None,
                file_size_kb: None,
                file_name: None,
                ingest_addr: None,
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
        let replay_addr = raw
            .replay_addr
            .unwrap_or_else(|| "0.0.0.0:7002".to_string());
        let poll_interval_ms = raw.poll_interval_ms.unwrap_or(250);

        let storage = match output.as_str() {
            "buffer" => StorageConfig::Buffer(BufferConfig {
                size_bytes: kib_to_bytes(raw.buffer_size_kb.unwrap_or(100))?,
                preserve_messages: raw.buffer_preserve.unwrap_or(0),
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
