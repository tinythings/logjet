use std::fs::File;
use std::io::{self, BufReader, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use logjet::{LogjetReader, ReaderConfig, RecordType};

use crate::config::CollectorConfig;
use crate::spool::list_named_segments;

pub fn replay_path_to_otlp_http(path: &Path, name: &str, collector: &CollectorConfig) -> io::Result<u64> {
    let mut sent = 0u64;
    let endpoint = CollectorEndpoint::parse(&collector.url)?;
    let timeout = Duration::from_millis(collector.timeout_ms);

    for segment in list_named_segments(path, name)? {
        let file = File::open(&segment.path)?;
        let mut reader = LogjetReader::with_config(BufReader::new(file), ReaderConfig::default());

        while let Some(record) = reader.next_record().map_err(to_io_error)? {
            if record.record_type != RecordType::Logs {
                continue;
            }

            post_raw_otlp_http(&endpoint, timeout, &record.payload)?;
            sent = sent.saturating_add(1);
        }
    }

    Ok(sent)
}

pub fn validate_replay_path(path: &Path, name: &str) -> io::Result<Vec<PathBuf>> {
    let segments = list_named_segments(path, name)?;
    Ok(segments.into_iter().map(|segment| segment.path).collect())
}

fn post_raw_otlp_http(endpoint: &CollectorEndpoint, timeout: Duration, payload: &[u8]) -> io::Result<()> {
    let mut stream = TcpStream::connect(&endpoint.authority)?;
    stream.set_write_timeout(Some(timeout))?;
    stream.set_read_timeout(Some(timeout))?;

    write!(
        stream,
        "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/x-protobuf\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        endpoint.path,
        endpoint.authority,
        payload.len()
    )?;
    stream.write_all(payload)?;
    stream.flush()?;

    let mut response = String::new();
    std::io::Read::read_to_string(&mut stream, &mut response)?;
    if !response.starts_with("HTTP/1.1 200") && !response.starts_with("HTTP/1.0 200") {
        return Err(io::Error::other(format!(
            "collector returned non-200 response: {}",
            response.lines().next().unwrap_or("unknown response")
        )));
    }

    Ok(())
}

fn to_io_error(err: logjet::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err.to_string())
}

struct CollectorEndpoint {
    authority: String,
    path: String,
}

impl CollectorEndpoint {
    fn parse(input: &str) -> io::Result<Self> {
        if let Some(rest) = input.strip_prefix("http://") {
            let (authority, path) = split_authority_and_path(rest);
            if authority.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "collector.url missing host:port",
                ));
            }
            return Ok(Self {
                authority: authority.to_string(),
                path: normalize_path(path),
            });
        }

        if input.starts_with("https://") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "https collector.url is not supported yet",
            ));
        }

        Ok(Self {
            authority: input.to_string(),
            path: "/v1/logs".to_string(),
        })
    }
}

fn split_authority_and_path(input: &str) -> (&str, &str) {
    match input.find('/') {
        Some(index) => (&input[..index], &input[index..]),
        None => (input, "/v1/logs"),
    }
}

fn normalize_path(path: &str) -> String {
    if path.is_empty() {
        "/v1/logs".to_string()
    } else if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}
