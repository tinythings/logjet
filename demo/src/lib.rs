use std::fmt::Write as _;
use std::fs;
use std::io::{self, Write as _};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use colored::Colorize;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope, KeyValue};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs, SeverityNumber};
use opentelemetry_proto::tonic::metrics::v1::number_data_point::Value as DataPointValue;
use opentelemetry_proto::tonic::metrics::v1::{
    AggregationTemporality, Gauge, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics, Sum,
};
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
    build_excuse_request_for_service_with_severity(sequence, "bofh-emitter", "warn")
}

pub fn build_excuse_request_for_service(sequence: u64, service_name: &str) -> ExportLogsServiceRequest {
    build_excuse_request_for_service_with_severity(sequence, service_name, "warn")
}

pub fn build_excuse_request_for_service_with_severity(sequence: u64, service_name: &str, severity: &str) -> ExportLogsServiceRequest {
    build_message_request_for_service(
        sequence,
        service_name,
        severity,
        format!("BOFH excuse #{sequence}: {}", BOFH_EXCUSES[(sequence as usize) % BOFH_EXCUSES.len()]),
    )
}

pub fn build_message_request(sequence: u64, body: String) -> ExportLogsServiceRequest {
    build_message_request_for_service(sequence, "bofh-emitter", "warn", body)
}

pub fn build_message_request_for_service(sequence: u64, service_name: &str, severity: &str, body: String) -> ExportLogsServiceRequest {
    let nanos = unix_time_nanos();
    let (severity_text, severity_number) = parse_demo_severity(severity);

    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: vec![
                    string_attr("service.name", service_name),
                    string_attr("service.namespace", "logjet-demo"),
                    string_attr("host.name", "garage-rig"),
                ],
                dropped_attributes_count: 0,
                entity_refs: Vec::new(),
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
                    severity_number,
                    severity_text,
                    body: Some(AnyValue { value: Some(opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(body)) }),
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

fn parse_demo_severity(value: &str) -> (String, i32) {
    match value {
        "trace" => ("TRACE".to_string(), SeverityNumber::Trace as i32),
        "debug" => ("DEBUG".to_string(), SeverityNumber::Debug as i32),
        "info" => ("INFO".to_string(), SeverityNumber::Info as i32),
        "warn" => ("WARN".to_string(), SeverityNumber::Warn as i32),
        "error" => ("ERROR".to_string(), SeverityNumber::Error as i32),
        "fatal" => ("FATAL".to_string(), SeverityNumber::Fatal as i32),
        _ => ("WARN".to_string(), SeverityNumber::Warn as i32),
    }
}

pub fn post_otlp_http(addr: &str, request: &ExportLogsServiceRequest) -> io::Result<()> {
    DemoConnection::open(addr, None, None)?.post(&request.encode_to_vec())
}

pub fn post_otlp_http_metrics(addr: &str, request: &ExportMetricsServiceRequest) -> io::Result<()> {
    DemoConnection::open(addr, None, None)?.post(&request.encode_to_vec())
}

pub fn build_metrics_request(sequence: u64) -> ExportMetricsServiceRequest {
    let nanos = unix_time_nanos();
    let cpu_value = 10.0 + ((sequence % 80) as f64) + (sequence % 7) as f64 * 0.5;
    let request_count = sequence * 100 + 42;

    let resource = Resource {
        attributes: vec![
            string_attr("service.name", "metrics-demo"),
            string_attr("host.name", "garage-rig"),
        ],
        dropped_attributes_count: 0,
        entity_refs: Vec::new(),
    };

    let scope = InstrumentationScope {
        name: "demo-metrics-emitter".to_string(),
        version: "0.1.0".to_string(),
        attributes: Vec::new(),
        dropped_attributes_count: 0,
    };

    let cpu_metric = Metric {
        name: "cpu.usage".to_string(),
        description: "Current CPU usage percentage".to_string(),
        unit: "%".to_string(),
        data: Some(opentelemetry_proto::tonic::metrics::v1::metric::Data::Gauge(Gauge {
            data_points: vec![NumberDataPoint {
                attributes: vec![string_attr("cpu", "all")],
                start_time_unix_nano: 0,
                time_unix_nano: nanos,
                value: Some(DataPointValue::AsDouble(cpu_value)),
                flags: 0,
                exemplars: Vec::new(),
            }],
        })),
        metadata: Vec::new(),
    };

    let requests_metric = Metric {
        name: "requests.total".to_string(),
        description: "Total number of requests served".to_string(),
        unit: "1".to_string(),
        data: Some(opentelemetry_proto::tonic::metrics::v1::metric::Data::Sum(Sum {
            data_points: vec![NumberDataPoint {
                attributes: vec![string_attr("method", "GET")],
                start_time_unix_nano: nanos - 1_000_000_000,
                time_unix_nano: nanos,
                value: Some(DataPointValue::AsInt(request_count as i64)),
                flags: 0,
                exemplars: Vec::new(),
            }],
            aggregation_temporality: AggregationTemporality::Cumulative as i32,
            is_monotonic: true,
        })),
        metadata: Vec::new(),
    };

    ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            resource: Some(resource),
            scope_metrics: vec![ScopeMetrics {
                scope: Some(scope),
                metrics: vec![cpu_metric, requests_metric],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    }
}

enum DemoStream {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
}

impl io::Read for DemoStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(s) => s.read(buf),
            Self::Tls(s) => s.read(buf),
        }
    }
}

impl io::Write for DemoStream {
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

/// Persistent HTTP/1.1 keep-alive connection for demo OTLP posting.
pub struct DemoConnection {
    stream: DemoStream,
    endpoint: DemoEndpoint,
    ca_file: Option<PathBuf>,
    override_server_name: Option<String>,
}

impl DemoConnection {
    pub fn open(addr: &str, ca_file: Option<&Path>, server_name: Option<&str>) -> io::Result<Self> {
        let endpoint = DemoEndpoint::parse(addr);
        let ca_owned = ca_file.map(Path::to_path_buf);
        let sn_owned = server_name.map(str::to_string);
        let stream = Self::connect_stream(&endpoint, ca_file, server_name)?;
        Ok(Self { stream, endpoint, ca_file: ca_owned, override_server_name: sn_owned })
    }

    fn connect_stream(endpoint: &DemoEndpoint, ca_file: Option<&Path>, server_name: Option<&str>) -> io::Result<DemoStream> {
        let tcp = TcpStream::connect(&endpoint.authority)?;
        tcp.set_write_timeout(Some(Duration::from_secs(5)))?;
        tcp.set_read_timeout(Some(Duration::from_secs(5)))?;
        if endpoint.tls {
            let ca =
                ca_file.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "https demo posting requires --ca-file or explicit CA path"))?;
            let cfg = load_demo_client_config(ca)?;
            let sn = demo_server_name(endpoint, server_name)?;
            let conn = ClientConnection::new(cfg, sn).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
            Ok(DemoStream::Tls(Box::new(StreamOwned::new(conn, tcp))))
        } else {
            Ok(DemoStream::Plain(tcp))
        }
    }

    fn reconnect(&mut self) -> io::Result<()> {
        self.stream = Self::connect_stream(&self.endpoint, self.ca_file.as_deref(), self.override_server_name.as_deref())?;
        Ok(())
    }

    /// POST one OTLP payload, reconnecting once on transport failure.
    pub fn post(&mut self, body: &[u8]) -> io::Result<()> {
        match self.post_inner(body) {
            Ok(()) => Ok(()),
            Err(_) => {
                self.reconnect()?;
                self.post_inner(body)
            }
        }
    }

    fn post_inner(&mut self, body: &[u8]) -> io::Result<()> {
        write!(
            self.stream,
            "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/x-protobuf\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
            self.endpoint.path,
            self.endpoint.authority,
            body.len()
        )?;
        self.stream.write_all(body)?;
        self.stream.flush()?;
        read_http_response(&mut self.stream)
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

pub fn load_demo_server_config(cert_path: &Path, key_path: &Path) -> io::Result<Arc<ServerConfig>> {
    let certs = load_certs(cert_path)?;
    let key = load_private_key(key_path)?;
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))?;
    Ok(Arc::new(config))
}

pub fn load_demo_mtls_server_config(ca_path: &Path, cert_path: &Path, key_path: &Path) -> io::Result<Arc<ServerConfig>> {
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

fn load_demo_client_config(ca_path: &Path) -> io::Result<Arc<ClientConfig>> {
    let roots = load_root_store(ca_path)?;
    Ok(Arc::new(ClientConfig::builder().with_root_certificates(roots).with_no_client_auth()))
}

fn demo_server_name(endpoint: &DemoEndpoint, override_name: Option<&str>) -> io::Result<ServerName<'static>> {
    let name = override_name.unwrap_or_else(|| endpoint.server_name());
    ServerName::try_from(name.to_string()).map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))
}

fn load_root_store(path: &Path) -> io::Result<RootCertStore> {
    let certs = load_certs(path)?;
    let mut roots = RootCertStore::empty();
    roots.add_parsable_certificates(certs);
    Ok(roots)
}

fn load_certs(path: &Path) -> io::Result<Vec<CertificateDer<'static>>> {
    let mut reader = std::io::BufReader::new(fs::File::open(path)?);
    rustls_pemfile::certs(&mut reader).collect::<Result<Vec<_>, _>>().map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))
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
            return Self { authority: authority.to_string(), path: normalise_path(path), tls: true };
        }

        if let Some(rest) = input.strip_prefix("http://") {
            let (authority, path) = split_authority_and_path(rest);
            return Self { authority: authority.to_string(), path: normalise_path(path), tls: false };
        }

        Self { authority: input.to_string(), path: "/v1/logs".to_string(), tls: false }
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
                resource.attributes.iter().find(|attr| attr.key == "service.name").and_then(|attr| attr.value.as_ref()).and_then(any_value_to_string)
            })
            .unwrap_or("unknown-service");

        for scope_logs in &resource_logs.scope_logs {
            let scope_name = scope_logs.scope.as_ref().map(|scope| scope.name.as_str()).unwrap_or("unknown-scope");

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
    let _ = writeln!(out, "service={} scope={} severity={} ts={}", service_name, scope_name, sev, log.time_unix_nano);
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
    if log.severity_text.is_empty() { "UNSPECIFIED" } else { log.severity_text.as_str() }
}

fn log_body(log: &LogRecord) -> &str {
    log.body.as_ref().and_then(any_value_to_string).unwrap_or("<no body>")
}

fn string_attr(key: &str, value: &str) -> KeyValue {
    KeyValue {
        key: key.to_string(),
        value: Some(AnyValue { value: Some(opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(value.to_string())) }),
    }
}

fn int_attr(key: &str, value: i64) -> KeyValue {
    KeyValue {
        key: key.to_string(),
        value: Some(AnyValue { value: Some(opentelemetry_proto::tonic::common::v1::any_value::Value::IntValue(value)) }),
    }
}

fn any_value_to_string(value: &AnyValue) -> Option<&str> {
    match value.value.as_ref()? {
        opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(value) => Some(value.as_str()),
        _ => None,
    }
}

fn unix_time_nanos() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos() as u64
}
