use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use logjet::{LogjetWriter, RecordType, WriterConfig};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope, KeyValue};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs, SeverityNumber};
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let output = match args.next() {
        Some(path) => PathBuf::from(path),
        None => {
            eprintln!("usage: cel-search-cli-logjet-generator <output.logjet> [count]");
            std::process::exit(2);
        }
    };
    let count = args.next().map(|value| value.parse::<u64>()).transpose()?.unwrap_or(200);

    let file = File::create(&output)?;
    let writer = BufWriter::new(file);
    let mut logjet = LogjetWriter::with_config(writer, WriterConfig::default());

    let services = ["core-api", "payment-worker", "auth-gateway", "event-bus", "cache-layer"];
    let severities: &[(&str, i32)] = &[
        ("DEBUG", SeverityNumber::Debug as i32),
        ("INFO", SeverityNumber::Info as i32),
        ("WARN", SeverityNumber::Warn as i32),
        ("ERROR", SeverityNumber::Error as i32),
        ("FATAL", SeverityNumber::Fatal as i32),
    ];
    let scopes = [
        "com.example.http-middleware",
        "com.example.payment-engine",
        "com.example.auth-module",
        "com.example.event-bus",
        "com.example.cache-manager",
    ];
    let regions = ["us-east-1", "us-east-1", "eu-west-1", "ap-southeast-1", "eu-west-1"];
    let envs = ["prod", "prod", "staging", "prod", "prod"];

    for seq in 1..=count {
        let idx = ((seq - 1) % 5) as usize;
        let service = services[idx];
        let (severity_text, severity_number) = severities[idx];
        let scope = scopes[idx];
        let region = regions[idx];
        let env = envs[idx];
        let ts_nanos = unix_time_nanos(seq);

        let (event_name, body, attrs) = build_event_data(idx, seq);

        let request = ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                resource: Some(Resource {
                    attributes: vec![
                        string_attr("service.name", service),
                        string_attr("service.namespace", "logjet-demo"),
                        string_attr("host.name", "garage-rig"),
                        string_attr("deploy.region", region),
                        string_attr("deploy.env", env),
                    ],
                    dropped_attributes_count: 0,
                    entity_refs: Vec::new(),
                }),
                scope_logs: vec![ScopeLogs {
                    scope: Some(InstrumentationScope {
                        name: scope.to_string(),
                        version: "0.1.0".to_string(),
                        attributes: vec![string_attr("library.language", "rust")],
                        dropped_attributes_count: 0,
                    }),
                    log_records: vec![LogRecord {
                        time_unix_nano: ts_nanos,
                        observed_time_unix_nano: ts_nanos,
                        severity_number,
                        severity_text: severity_text.to_string(),
                        body: Some(AnyValue {
                            value: Some(opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(
                                body.to_string(),
                            )),
                        }),
                        attributes: attrs,
                        dropped_attributes_count: 0,
                        flags: 0,
                        trace_id: Vec::new(),
                        span_id: Vec::new(),
                        event_name: event_name.to_string(),
                    }],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        };

        logjet.push(RecordType::Logs, seq, ts_nanos, &request.encode_to_vec())?;
    }

    let mut writer = logjet.into_inner()?;
    writer.flush()?;
    println!("wrote {count} log records to {}", output.display());
    Ok(())
}

fn pivot(seq: u64) -> usize {
    ((seq - 1) / 5) as usize
}

fn build_event_data(idx: usize, seq: u64) -> (&'static str, String, Vec<KeyValue>) {
    match idx {
        0 => http_request(seq),
        1 => payment_process(seq),
        2 => auth_login(seq),
        3 => event_bus(seq),
        _ => cache_access(seq),
    }
}

fn http_request(seq: u64) -> (&'static str, String, Vec<KeyValue>) {
    let bodies: &[(&str, i64, &str, &str)] = &[
        ("GET /api/items returned 200 in 12ms", 200, "GET", "/api/items"),
        ("POST /api/orders returned 201 in 45ms", 201, "POST", "/api/orders"),
        ("GET /api/items returned 500: internal server error", 500, "GET", "/api/items"),
        ("DELETE /api/sessions returned 404: not found", 404, "DELETE", "/api/sessions"),
        ("POST /api/users returned 200 in 8ms", 200, "POST", "/api/users"),
        ("GET /api/health returned 200 in 1ms", 200, "GET", "/api/health"),
        ("GET /api/orders returned 504: gateway timeout", 504, "GET", "/api/orders"),
        ("POST /api/items returned 500: internal server error", 500, "POST", "/api/items"),
        ("GET /api/users returned 403: forbidden", 403, "GET", "/api/users"),
        ("DELETE /api/sessions returned 200 in 5ms", 200, "DELETE", "/api/sessions"),
    ];
    let (body, status, method, route) = bodies[pivot(seq) % bodies.len()];
    let dur = 1 + (seq % 100);
    let mut attrs = vec![
        string_attr("http.method", method),
        string_attr("http.route", route),
        int_attr("http.status_code", status),
        int_attr("http.duration_ms", dur as i64),
    ];
    if status >= 500 {
        attrs.push(string_attr("error.code", "INTERNAL_ERROR"));
    } else if status >= 400 {
        attrs.push(string_attr("error.code", "CLIENT_ERROR"));
    }
    ("http.request", body.to_string(), attrs)
}

fn payment_process(seq: u64) -> (&'static str, String, Vec<KeyValue>) {
    let amounts = [19.99, 45.50, 30.00, 12.75, 99.99, 22.00, 5.00, 18.50, 67.30, 41.20];
    let templates: &[&str] = &[
        "payment for order #{} succeeded: amount $19.99",
        "payment for order #{} succeeded: amount $45.50",
        "payment for order #{} failed: timeout",
        "payment for order #{} failed: insufficient funds",
        "payment for order #{} succeeded: amount $99.99",
        "payment for order #{} failed: gateway unavailable",
        "payment for order #{} succeeded: amount $5.00",
        "payment for order #{} failed: timeout",
        "payment for order #{} succeeded: amount $67.30",
        "payment for order #{} failed: rate limited",
    ];
    let i = pivot(seq) % templates.len();
    let order_id = seq * 100 + 42;
    let body = templates[i].replace("{}", &order_id.to_string());
    let is_ok = !body.contains("failed");
    let mut attrs = vec![
        int_attr("order.id", order_id as i64),
        double_attr("payment.amount", amounts[i]),
    ];
    if !is_ok {
        attrs.push(string_attr("error.code", "PAYMENT_FAILED"));
    }
    ("payment.process", body, attrs)
}

fn auth_login(seq: u64) -> (&'static str, String, Vec<KeyValue>) {
    let users = ["bob@example.com", "alice@corp.io", "ops@startup.dev", "dev@local.host", "admin@system.org"];
    let templates: &[(&str, &str)] = &[
        ("user {} logged in via password", "password"),
        ("user {} logged in via oauth2", "oauth2"),
        ("user {} login failed: invalid credentials", "password"),
        ("user {} logged in via sso", "sso"),
        ("user {} logged in via token", "token"),
        ("user {} login failed: expired session", "token"),
        ("user {} logged in via password", "password"),
        ("user {} login failed: invalid credentials", "password"),
        ("user {} logged in via oauth2", "oauth2"),
        ("user {} logged in via sso", "sso"),
    ];
    let i = pivot(seq) % templates.len();
    let (template, method) = templates[i];
    let user = users[pivot(seq) % users.len()];
    let body = template.replace("{}", user);
    let mut attrs = vec![
        string_attr("user.id", user),
        string_attr("auth.method", method),
        int_attr("auth.session_ttl", 3600),
    ];
    if body.contains("failed") {
        attrs.push(string_attr("error.code", "AUTH_FAILED"));
    }
    ("auth.login", body, attrs)
}

fn event_bus(seq: u64) -> (&'static str, String, Vec<KeyValue>) {
    let topics = ["order.created", "payment.completed", "user.registered", "cache.invalidated", "email.sent"];
    let templates: &[&str] = &[
        "event bus processed message on topic {} ok",
        "event bus processed message on topic {} ok",
        "event bus consumer lag high on topic {}: 3500ms",
        "event bus processed message on topic {} ok",
        "event bus consumer lag critical on topic {}: 12000ms behind",
        "event bus processed message on topic {} ok",
        "event bus processed message on topic {} ok",
        "event bus consumer lag high on topic {}: 4200ms",
        "event bus consumer lag critical on topic {}: 9500ms behind",
        "event bus processed message on topic {} ok",
    ];
    let i = pivot(seq) % templates.len();
    let topic = topics[pivot(seq) % topics.len()];
    let body = templates[i].replace("{}", topic);
    let mut attrs = vec![string_attr("messaging.topic", topic)];
    if body.contains("critical") {
        attrs.push(int_attr("messaging.consumer_lag", 12000));
        attrs.push(string_attr("error.code", "CONSUMER_LAG_CRITICAL"));
    } else if body.contains("high") {
        attrs.push(int_attr("messaging.consumer_lag", 3500));
        attrs.push(string_attr("error.code", "CONSUMER_LAG_HIGH"));
    } else {
        attrs.push(int_attr("messaging.consumer_lag", 10));
    }
    ("message.processed", body, attrs)
}

fn cache_access(seq: u64) -> (&'static str, String, Vec<KeyValue>) {
    let keys = [
        "user:session:172",
        "product:inventory:55",
        "config:feature-flags",
        "rate-limit:api:bucket-3",
        "db:query:recent-orders",
    ];
    let templates: &[&str] = &[
        "cache hit on key {} (ttl 300s)",
        "cache hit on key {} (ttl 450s)",
        "cache miss on key {}",
        "cache hit on key {} (ttl 380s)",
        "cache miss on key {}",
        "cache hit on key {} (ttl 600s)",
        "cache hit on key {} (ttl 120s)",
        "cache miss on key {}",
        "cache hit on key {} (ttl 420s)",
        "cache hit on key {} (ttl 200s)",
    ];
    let i = pivot(seq) % templates.len();
    let key = keys[pivot(seq) % keys.len()];
    let body = templates[i].replace("{}", key);
    let ttl: i64 = if body.contains("miss") { 0 } else { 300 };
    let mut attrs = vec![string_attr("cache.key", key), int_attr("cache.ttl", ttl)];
    if body.contains("miss") {
        attrs.push(string_attr("error.code", "CACHE_MISS"));
    }
    ("cache.access", body, attrs)
}

fn string_attr(key: &str, value: &str) -> KeyValue {
    KeyValue {
        key: key.to_string(),
        value: Some(AnyValue {
            value: Some(opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(value.to_string())),
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

fn double_attr(key: &str, value: f64) -> KeyValue {
    KeyValue {
        key: key.to_string(),
        value: Some(AnyValue {
            value: Some(opentelemetry_proto::tonic::common::v1::any_value::Value::DoubleValue(value)),
        }),
    }
}

fn unix_time_nanos(seq: u64) -> u64 {
    let base = 1_773_000_000_000_000_000u64;
    base.saturating_add(seq.saturating_mul(90_000_000_000))
}
