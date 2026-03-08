use std::fmt::Write as _;
use std::fs;
use std::io;
use std::net::TcpStream;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use colored::Colorize;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope, KeyValue};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs, SeverityNumber};
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, ServerConfig, StreamOwned};

const BOFH_EXCUSES: &[&str] = &[
    "clock skew from a hostile NTP daemon",
    "magnetic interference from a mislabeled coffee mug",
    "temporary routing loop caused by an intern with initiative",
    "cosmic rays flipped the wrong bit again",
    "the backup fan controller achieved sentience and quit",
    "kernel panic triggered by excessive optimism",
    "database latency due to an emotionally unavailable SAN",
    "a consultant enabled enterprise mode",
    "heat death postponed packet delivery",
    "the packet inspector was inspected and found wanting",
    "firmware entered a spiritual debugging journey",
    "someone rebooted the wrong reality",
];

pub fn build_excuse_request(sequence: u64) -> ExportLogsServiceRequest {
    build_excuse_request_for_service(sequence, "bofh-emitter")
}

pub fn build_excuse_request_for_service(
    sequence: u64,
    service_name: &str,
) -> ExportLogsServiceRequest {
    build_message_request_for_service(
        sequence,
        service_name,
        format!(
            "BOFH excuse #{sequence}: {}",
            BOFH_EXCUSES[(sequence as usize) % BOFH_EXCUSES.len()]
        ),
    )
}

pub fn build_message_request(sequence: u64, body: String) -> ExportLogsServiceRequest {
    build_message_request_for_service(sequence, "bofh-emitter", body)
}

pub fn build_message_request_for_service(
    sequence: u64,
    service_name: &str,
    body: String,
) -> ExportLogsServiceRequest {
    let nanos = unix_time_nanos();

    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: vec![
                    string_attr("service.name", service_name),
                    string_attr("service.namespace", "logjet-demo"),
                    string_attr("host.name", "garage-rig"),
                ],
                dropped_attributes_count: 0,
            }),
            scope_logs: vec![ScopeLogs {
                scope: Some(InstrumentationScope {
                    name: "logjet-demo-emitter".to_string(),
                    version: "0.1.0".to_string(),
                    attributes: Vec::new(),
                    dropped_attributes_count: 0,
                }),
                log_records: vec![LogRecord {
                    time_unix_nano: nanos,
                    observed_time_unix_nano: nanos,
                    severity_number: SeverityNumber::Warn as i32,
                    severity_text: "WARN".to_string(),
                    body: Some(AnyValue {
                        value: Some(opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(
                            body,
                        )),
                    }),
                    attributes: vec![
                        string_attr("demo.kind", "bofh"),
                        string_attr("demo.component", "emitter"),
                        string_attr("event.domain", "logs"),
                        int_attr("demo.sequence", sequence as i64),
                    ],
                    dropped_attributes_count: 0,
                    flags: 0,
                    trace_id: Vec::new(),
                    span_id: Vec::new(),
                    event_name: "bofh.excuse".to_string(),
                }],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    }
}

pub fn post_otlp_http(addr: &str, request: &ExportLogsServiceRequest) -> io::Result<()> {
    post_raw_otlp_http(addr, &request.encode_to_vec(), None, None)
}

pub fn post_raw_otlp_http(
    addr: &str,
    body: &[u8],
    ca_file: Option<&Path>,
    server_name: Option<&str>,
) -> io::Result<()> {
    let endpoint = DemoEndpoint::parse(addr);
    let mut stream = TcpStream::connect(&endpoint.authority)?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;

    if endpoint.tls {
        let ca_file = ca_file.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "https demo posting requires --ca-file or explicit CA path",
            )
        })?;
        let client_config = load_demo_client_config(ca_file)?;
        let server_name = demo_server_name(&endpoint, server_name)?;
        let conn = ClientConnection::new(client_config, server_name)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))?;
        let mut transport = StreamOwned::new(conn, stream);
        return post_raw_otlp_http_transport(&endpoint, body, &mut transport);
    }

    post_raw_otlp_http_transport(&endpoint, body, &mut stream)
}

pub fn load_demo_server_config(cert_path: &Path, key_path: &Path) -> io::Result<Arc<ServerConfig>> {
    let certs = load_certs(cert_path)?;
    let key = load_private_key(key_path)?;
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))?;
    Ok(Arc::new(config))
}

pub fn load_demo_mtls_server_config(
    ca_path: &Path,
    cert_path: &Path,
    key_path: &Path,
) -> io::Result<Arc<ServerConfig>> {
    let certs = load_certs(cert_path)?;
    let key = load_private_key(key_path)?;
    let roots = load_root_store(ca_path)?;
    let verifier = rustls::server::WebPkiClientVerifier::builder(Arc::new(roots))
        .build()
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))?;
    let config = ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(certs, key)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))?;
    Ok(Arc::new(config))
}

fn post_raw_otlp_http_transport<T: io::Read + io::Write>(
    endpoint: &DemoEndpoint,
    body: &[u8],
    transport: &mut T,
) -> io::Result<()> {
    write!(
        transport,
        "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/x-protobuf\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        endpoint.path,
        endpoint.authority,
        body.len()
    )?;
    transport.write_all(body)?;
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

fn load_demo_client_config(ca_path: &Path) -> io::Result<Arc<ClientConfig>> {
    let roots = load_root_store(ca_path)?;
    Ok(Arc::new(
        ClientConfig::builder().with_root_certificates(roots).with_no_client_auth(),
    ))
}

fn demo_server_name(endpoint: &DemoEndpoint, override_name: Option<&str>) -> io::Result<ServerName<'static>> {
    let name = override_name.unwrap_or_else(|| endpoint.server_name());
    ServerName::try_from(name.to_string())
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))
}

fn load_root_store(path: &Path) -> io::Result<RootCertStore> {
    let certs = load_certs(path)?;
    let mut roots = RootCertStore::empty();
    roots.add_parsable_certificates(certs);
    Ok(roots)
}

fn load_certs(path: &Path) -> io::Result<Vec<CertificateDer<'static>>> {
    let mut reader = std::io::BufReader::new(fs::File::open(path)?);
    rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))
}

fn load_private_key(path: &Path) -> io::Result<PrivateKeyDer<'static>> {
    let mut reader = std::io::BufReader::new(fs::File::open(path)?);
    rustls_pemfile::private_key(&mut reader)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))?
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing private key"))
}

pub struct DemoEndpoint {
    authority: String,
    path: String,
    tls: bool,
}

impl DemoEndpoint {
    fn parse(input: &str) -> Self {
        if let Some(rest) = input.strip_prefix("https://") {
            let (authority, path) = split_authority_and_path(rest);
            return Self {
                authority: authority.to_string(),
                path: normalise_path(path),
                tls: true,
            };
        }

        if let Some(rest) = input.strip_prefix("http://") {
            let (authority, path) = split_authority_and_path(rest);
            return Self {
                authority: authority.to_string(),
                path: normalise_path(path),
                tls: false,
            };
        }

        Self {
            authority: input.to_string(),
            path: "/v1/logs".to_string(),
            tls: false,
        }
    }

    fn server_name(&self) -> &str {
        self.authority.rsplit_once(':').map(|(host, _)| host).unwrap_or(&self.authority)
    }
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

pub fn format_batch_plain(batch: &ExportLogsServiceRequest) -> String {
    format_batch(batch, false)
}

pub fn format_batch_coloured(batch: &ExportLogsServiceRequest) -> String {
    format_batch(batch, true)
}

fn format_batch(batch: &ExportLogsServiceRequest, coloured: bool) -> String {
    let mut out = String::new();

    for resource_logs in &batch.resource_logs {
        let service_name = resource_logs
            .resource
            .as_ref()
            .and_then(|resource| {
                resource
                    .attributes
                    .iter()
                    .find(|attr| attr.key == "service.name")
                    .and_then(|attr| attr.value.as_ref())
                    .and_then(any_value_to_string)
            })
            .unwrap_or("unknown-service");

        for scope_logs in &resource_logs.scope_logs {
            let scope_name = scope_logs
                .scope
                .as_ref()
                .map(|scope| scope.name.as_str())
                .unwrap_or("unknown-scope");

            for log in &scope_logs.log_records {
                if coloured {
                    write_coloured_record(&mut out, service_name, scope_name, log);
                } else {
                    write_plain_record(&mut out, service_name, scope_name, log);
                }
            }
        }
    }

    out
}

fn write_plain_record(out: &mut String, service_name: &str, scope_name: &str, log: &LogRecord) {
    let sev = severity_text(log);
    let body = log_body(log);
    let _ = writeln!(
        out,
        "service={} scope={} severity={} ts={}",
        service_name, scope_name, sev, log.time_unix_nano
    );
    let _ = writeln!(out, "message: {body}");
    let _ = writeln!(out);
}

fn write_coloured_record(out: &mut String, service_name: &str, scope_name: &str, log: &LogRecord) {
    let sev = severity_text(log);
    let body = log_body(log);
    let _ = writeln!(
        out,
        "{}{}{}{}{}{}{}{}",
        "service=".bright_blue().bold(),
        service_name,
        " scope=".bright_magenta().bold(),
        scope_name,
        " severity=".bright_yellow().bold(),
        sev,
        " ts=".bright_green().bold(),
        log.time_unix_nano
    );
    let _ = writeln!(out, "{} {}", "message:".bright_cyan().bold(), body);
    let _ = writeln!(out);
}

fn severity_text(log: &LogRecord) -> &str {
    if log.severity_text.is_empty() {
        "UNSPECIFIED"
    } else {
        log.severity_text.as_str()
    }
}

fn log_body(log: &LogRecord) -> &str {
    log.body
        .as_ref()
        .and_then(any_value_to_string)
        .unwrap_or("<no body>")
}

fn string_attr(key: &str, value: &str) -> KeyValue {
    KeyValue {
        key: key.to_string(),
        value: Some(AnyValue {
            value: Some(opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(
                value.to_string(),
            )),
        }),
    }
}

fn int_attr(key: &str, value: i64) -> KeyValue {
    KeyValue {
        key: key.to_string(),
        value: Some(AnyValue {
            value: Some(opentelemetry_proto::tonic::common::v1::any_value::Value::IntValue(value)),
        }),
    }
}

fn any_value_to_string(value: &AnyValue) -> Option<&str> {
    match value.value.as_ref()? {
        opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(value) => Some(value.as_str()),
        _ => None,
    }
}

fn unix_time_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}
