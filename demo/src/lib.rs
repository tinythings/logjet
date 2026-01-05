use std::fmt::Write as _;
use std::io::{self, Write};
use std::net::TcpStream;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use colored::Colorize;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope, KeyValue};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs, SeverityNumber};
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message;

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
    let nanos = unix_time_nanos();
    let body = BOFH_EXCUSES[(sequence as usize) % BOFH_EXCUSES.len()];

    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: vec![
                    string_attr("service.name", "bofh-emitter"),
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
                            format!("BOFH excuse #{sequence}: {body}"),
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
    let mut stream = TcpStream::connect(addr)?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;

    let body = request.encode_to_vec();
    write!(
        stream,
        "POST /v1/logs HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/x-protobuf\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(&body)?;
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

pub fn format_batch_plain(batch: &ExportLogsServiceRequest) -> String {
    format_batch(batch, false)
}

pub fn format_batch_colored(batch: &ExportLogsServiceRequest) -> String {
    format_batch(batch, true)
}

fn format_batch(batch: &ExportLogsServiceRequest, colored: bool) -> String {
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
                if colored {
                    write_colored_record(&mut out, service_name, scope_name, log);
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

fn write_colored_record(out: &mut String, service_name: &str, scope_name: &str, log: &LogRecord) {
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
