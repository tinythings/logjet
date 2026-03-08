use std::fs::File;
use std::io::{self, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use logjet::{LogjetReader, ReaderConfig, RecordType};
use rustls::{ClientConfig, ClientConnection, StreamOwned};

use crate::config::{CollectorConfig, TlsConfig, UpstreamConfig};
use crate::protocol::{ReplayRequest, read_record, write_replay_request};
use crate::spool::list_named_segments;
use crate::tls::{load_client_config, parse_server_name};

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

pub fn bridge_wire_to_otlp_http(
    source: &str,
    collector: &CollectorConfig,
    upstream: &UpstreamConfig,
    tls: &TlsConfig,
) -> io::Result<()> {
    let endpoint = CollectorEndpoint::parse(&collector.url)?;
    let collector_timeout = Duration::from_millis(collector.timeout_ms);
    let connect_timeout = Duration::from_millis(upstream.connect_timeout_ms);
    let retry_delay = Duration::from_millis(upstream.retry_ms);
    let tls_client = if tls.enable {
        Some(load_client_config(tls)?)
    } else {
        None
    };
    let mut last_seq = 0u64;

    loop {
        match bridge_once(
            source,
            &endpoint,
            collector_timeout,
            connect_timeout,
            &mut last_seq,
            tls,
            tls_client.clone(),
        ) {
            Ok(()) => {
                eprintln!(
                    "bridge source {source} closed after seq={last_seq}; reconnecting in {} ms",
                    upstream.retry_ms
                );
            }
            Err(err) => {
                eprintln!(
                    "bridge source {source} error after seq={last_seq}: {err}; reconnecting in {} ms",
                    upstream.retry_ms
                );
            }
        }
        thread::sleep(retry_delay);
    }
}

fn bridge_once(
    source: &str,
    endpoint: &CollectorEndpoint,
    collector_timeout: Duration,
    connect_timeout: Duration,
    last_seq: &mut u64,
    tls: &TlsConfig,
    tls_client: Option<Arc<ClientConfig>>,
) -> io::Result<()> {
    let stream = connect_with_timeout(source, connect_timeout)?;
    stream.set_read_timeout(None)?;
    stream.set_write_timeout(Some(connect_timeout))?;

    if let Some(client_config) = tls_client {
        let server_name = parse_server_name(tls, source)?;
        let conn = ClientConnection::new(client_config, server_name)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))?;
        let mut transport = StreamOwned::new(conn, stream);
        return bridge_transport(source, endpoint, collector_timeout, last_seq, &mut transport);
    }

    let mut transport = stream;
    bridge_transport(source, endpoint, collector_timeout, last_seq, &mut transport)
}

fn bridge_transport<T: io::Read + io::Write>(
    source: &str,
    endpoint: &CollectorEndpoint,
    collector_timeout: Duration,
    last_seq: &mut u64,
    transport: &mut T,
) -> io::Result<()> {
    write_replay_request(transport, &ReplayRequest { from_seq: *last_seq })?;
    transport.flush()?;
    eprintln!(
        "bridge connected to {} and requested records after seq={}",
        source, *last_seq
    );

    while let Some(record) = read_record(transport)? {
        if record.record_type == RecordType::Logs {
            post_raw_otlp_http(endpoint, collector_timeout, &record.payload)?;
        }
        *last_seq = record.seq;
    }

    Ok(())
}

fn post_raw_otlp_http(endpoint: &CollectorEndpoint, timeout: Duration, payload: &[u8]) -> io::Result<()> {
    let mut stream = connect_with_timeout(&endpoint.authority, timeout)?;
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

fn connect_with_timeout(authority: &str, timeout: Duration) -> io::Result<TcpStream> {
    let mut last_err = None;
    for addr in authority.to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(stream) => return Ok(stream),
            Err(err) => last_err = Some(err),
        }
    }

    Err(last_err.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("collector or upstream address could not be resolved: {authority}"),
        )
    }))
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

#[cfg(test)]
#[path = "replay_utst.rs"]
mod replay_utst;
