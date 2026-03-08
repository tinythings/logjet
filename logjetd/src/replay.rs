use std::fs::{self, File};
use std::io::{self, BufReader};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use logjet::{LogjetReader, ReaderConfig, RecordType};
use rustls::{ClientConfig, ClientConnection, StreamOwned};

use crate::config::{BackpressureConfig, BackpressureMode, CollectorConfig, TlsConfig, UpstreamConfig, UpstreamMode};
use crate::protocol::{
    ReplayAck, ReplayHello, ReplayRequest, read_record, read_replay_hello, write_replay_ack,
    write_replay_request,
};
use crate::spool::list_named_segments;
use crate::tls::{load_client_config, load_collector_client_config, parse_collector_server_name, parse_server_name};

pub fn replay_path_to_otlp_http(path: &Path, name: &str, collector: &CollectorConfig) -> io::Result<u64> {
    let mut sent = 0u64;
    let endpoint = CollectorEndpoint::parse(&collector.url)?;
    let transport = CollectorTransport {
        timeout: Duration::from_millis(collector.timeout_ms),
        backpressure_enabled: false,
        backpressure_mode: BackpressureMode::Disconnect,
        tls_client: if endpoint.tls {
            Some(load_collector_client_config(collector)?)
        } else {
            None
        },
        endpoint,
        collector: collector.clone(),
        upstream_mode: UpstreamMode::Keep,
    };

    for segment in list_named_segments(path, name)? {
        let file = File::open(&segment.path)?;
        let mut reader = LogjetReader::with_config(BufReader::new(file), ReaderConfig::default());

        while let Some(record) = reader.next_record().map_err(to_io_error)? {
            if record.record_type != RecordType::Logs {
                continue;
            }

            post_raw_otlp_http(&transport, &record.payload)?;
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
    backpressure: &BackpressureConfig,
    upstream: &UpstreamConfig,
    tls: &TlsConfig,
) -> io::Result<()> {
    let endpoint = CollectorEndpoint::parse(&collector.url)?;
    let connect_timeout = Duration::from_millis(upstream.connect_timeout_ms);
    let retry_delay = Duration::from_millis(upstream.retry_ms);
    let tls_client = if tls.enable {
        Some(load_client_config(tls)?)
    } else {
        None
    };
    let collector_transport = CollectorTransport {
        timeout: Duration::from_millis(collector.timeout_ms),
        backpressure_enabled: backpressure.enabled,
        backpressure_mode: backpressure.mode,
        tls_client: if endpoint.tls {
            Some(load_collector_client_config(collector)?)
        } else {
            None
        },
        endpoint,
        collector: collector.clone(),
        upstream_mode: upstream.mode,
    };
    let mut state = read_bridge_state(upstream.state_file.as_deref())?;
    if let Some(path) = upstream.state_file.as_deref() {
        eprintln!(
            "bridge resume state file {} loaded seq={} stream-id={}",
            path.display(),
            state.last_seq,
            state.stream_id
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unset".to_string())
        );
    }

    loop {
        match bridge_once(
            source,
            connect_timeout,
            &mut state,
            upstream.state_file.as_deref(),
            tls,
            tls_client.clone(),
            &collector_transport,
        ) {
            Ok(()) => {
                eprintln!(
                    "bridge source {source} closed after seq={}; reconnecting in {} ms",
                    state.last_seq,
                    upstream.retry_ms
                );
            }
            Err(err) => {
                eprintln!(
                    "bridge source {source} error after seq={}: {err}; reconnecting in {} ms",
                    state.last_seq,
                    upstream.retry_ms
                );
            }
        }
        thread::sleep(retry_delay);
    }
}

fn bridge_once(
    source: &str,
    connect_timeout: Duration,
    state: &mut BridgeState,
    state_file: Option<&Path>,
    tls: &TlsConfig,
    tls_client: Option<Arc<ClientConfig>>,
    collector_transport: &CollectorTransport,
) -> io::Result<()> {
    let stream = connect_with_timeout(source, connect_timeout)?;
    stream.set_read_timeout(None)?;
    stream.set_write_timeout(Some(connect_timeout))?;

    if let Some(client_config) = tls_client {
        let server_name = parse_server_name(tls, source)?;
        let conn = ClientConnection::new(client_config, server_name)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))?;
        let mut transport = StreamOwned::new(conn, stream);
        return bridge_transport(source, state, state_file, &mut transport, collector_transport);
    }

    let mut transport = stream;
    bridge_transport(source, state, state_file, &mut transport, collector_transport)
}

fn bridge_transport<T: io::Read + io::Write>(
    source: &str,
    state: &mut BridgeState,
    state_file: Option<&Path>,
    transport: &mut T,
    collector_transport: &CollectorTransport,
) -> io::Result<()> {
    let hello = read_replay_hello(transport)?;
    reconcile_bridge_state(source, state, &hello)?;
    write_bridge_state(state_file, state)?;

    let consume = collector_transport.upstream_mode == UpstreamMode::Drain;
    write_replay_request(transport, &ReplayRequest { from_seq: state.last_seq, consume })?;
    transport.flush()?;
    eprintln!(
        "bridge connected to {} and requested records after seq={} mode={}",
        source,
        state.last_seq,
        if consume { "drain" } else { "keep" }
    );

    while let Some(record) = read_record(transport)? {
        if record.record_type == RecordType::Logs {
            post_raw_otlp_http(collector_transport, &record.payload)?;
        }
        state.last_seq = record.seq;
        write_bridge_state(state_file, state)?;
        if consume {
            write_replay_ack(transport, &ReplayAck { ack_seq: record.seq })?;
            transport.flush()?;
        }
    }

    Ok(())
}

fn post_raw_otlp_http(collector_transport: &CollectorTransport, payload: &[u8]) -> io::Result<()> {
    let stream = connect_with_timeout(&collector_transport.endpoint.authority, collector_transport.timeout)?;
    if !collector_transport.backpressure_enabled {
        stream.set_write_timeout(Some(collector_transport.timeout))?;
        stream.set_read_timeout(Some(collector_transport.timeout))?;
    } else {
        match collector_transport.backpressure_mode {
            BackpressureMode::Block => {
                stream.set_write_timeout(None)?;
                stream.set_read_timeout(None)?;
            }
            BackpressureMode::Disconnect => {
                stream.set_write_timeout(Some(collector_transport.timeout))?;
                stream.set_read_timeout(Some(collector_transport.timeout))?;
            }
        }
    }

    if let Some(client_config) = &collector_transport.tls_client {
        let server_name = parse_collector_server_name(
            &collector_transport.collector,
            &collector_transport.endpoint.authority,
        )?;
        let conn = ClientConnection::new(client_config.clone(), server_name)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))?;
        let mut tls_transport = StreamOwned::new(conn, stream);
        return post_raw_otlp_http_transport(&collector_transport.endpoint, payload, &mut tls_transport);
    }

    let mut plain_transport = stream;
    post_raw_otlp_http_transport(&collector_transport.endpoint, payload, &mut plain_transport)
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct BridgeState {
    stream_id: Option<u64>,
    last_seq: u64,
}

fn read_bridge_state(path: Option<&Path>) -> io::Result<BridgeState> {
    let Some(path) = path else {
        return Ok(BridgeState {
            stream_id: None,
            last_seq: 0,
        });
    };
    if !path.exists() {
        return Ok(BridgeState {
            stream_id: None,
            last_seq: 0,
        });
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
        format!(
            "stream_id={}\nlast_seq={}\n",
            state
                .stream_id
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unset".to_string()),
            state.last_seq
        ),
    )
}

fn parse_bridge_state(text: &str) -> io::Result<BridgeState> {
    if let Ok(last_seq) = text.trim().parse::<u64>() {
        return Ok(BridgeState {
            stream_id: None,
            last_seq,
        });
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
                    Some(Some(value.parse::<u64>().map_err(|err| {
                        io::Error::new(io::ErrorKind::InvalidData, format!("invalid bridge state stream_id: {err}"))
                    })?))
                };
            }
            "last_seq" => {
                last_seq = Some(value.trim().parse::<u64>().map_err(|err| {
                    io::Error::new(io::ErrorKind::InvalidData, format!("invalid bridge state last_seq: {err}"))
                })?);
            }
            _ => {}
        }
    }

    let last_seq = last_seq
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid bridge state: missing last_seq"))?;
    Ok(BridgeState {
        stream_id: stream_id.unwrap_or(None),
        last_seq,
    })
}

fn reconcile_bridge_state(source: &str, state: &mut BridgeState, hello: &ReplayHello) -> io::Result<()> {
    if let Some(saved_stream_id) = state.stream_id {
        if saved_stream_id != hello.stream_id {
            eprintln!(
                "bridge source {source} changed stream identity {} -> {}; resetting saved seq from {} to 0",
                saved_stream_id,
                hello.stream_id,
                state.last_seq
            );
            state.last_seq = 0;
        }
    } else if state.last_seq > 0 && hello.last_seq > 0 && hello.last_seq < state.last_seq {
        eprintln!(
            "bridge source {source} appears to have reset or been replaced; upstream last_seq={} is below saved seq={}; resetting to 0",
            hello.last_seq,
            state.last_seq
        );
        state.last_seq = 0;
    }
    state.stream_id = Some(hello.stream_id);
    Ok(())
}

struct CollectorEndpoint {
    authority: String,
    path: String,
    tls: bool,
}

struct CollectorTransport {
    endpoint: CollectorEndpoint,
    timeout: Duration,
    backpressure_enabled: bool,
    backpressure_mode: BackpressureMode,
    tls_client: Option<Arc<ClientConfig>>,
    collector: CollectorConfig,
    upstream_mode: UpstreamMode,
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
                path: normalise_path(path),
                tls: false,
            });
        }

        if let Some(rest) = input.strip_prefix("https://") {
            let (authority, path) = split_authority_and_path(rest);
            if authority.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "collector.url missing host:port",
                ));
            }
            return Ok(Self {
                authority: authority.to_string(),
                path: normalise_path(path),
                tls: true,
            });
        }

        Ok(Self {
            authority: input.to_string(),
            path: "/v1/logs".to_string(),
            tls: false,
        })
    }
}

fn post_raw_otlp_http_transport<T: io::Read + io::Write>(
    endpoint: &CollectorEndpoint,
    payload: &[u8],
    transport: &mut T,
) -> io::Result<()> {
    write!(
        transport,
        "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/x-protobuf\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        endpoint.path,
        endpoint.authority,
        payload.len()
    )?;
    transport.write_all(payload)?;
    transport.flush()?;

    let mut response = String::new();
    std::io::Read::read_to_string(transport, &mut response)?;
    if !response.starts_with("HTTP/1.1 200") && !response.starts_with("HTTP/1.0 200") {
        return Err(io::Error::other(format!(
            "collector returned non-200 response: {}",
            response.lines().next().unwrap_or("unknown response")
        )));
    }

    Ok(())
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

#[cfg(test)]
#[path = "replay_utst.rs"]
mod replay_utst;
