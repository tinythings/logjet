use std::env;
use std::io::{self, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use prost::Message;
use rustls::{ServerConnection, StreamOwned};
use tiny_http::{Header, Method, Response, Server, StatusCode};

use otlp_demo::{format_batch_coloured, load_demo_server_config};

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut bind_addr = "0.0.0.0:4318".to_string();
    let mut tls = false;
    let mut cert_file = None;
    let mut key_file = None;
    let mut delay_ms = 0u64;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--tls" => tls = true,
            "--delay-ms" => {
                delay_ms = args.next().ok_or("missing value for --delay-ms")?.parse::<u64>()?;
            }
            "--cert-file" => {
                cert_file = Some(PathBuf::from(args.next().ok_or("missing value for --cert-file")?));
            }
            "--key-file" => {
                key_file = Some(PathBuf::from(args.next().ok_or("missing value for --key-file")?));
            }
            value => bind_addr = value.to_string(),
        }
    }
    if tls {
        return run_tls(&bind_addr, cert_file, key_file, delay_ms);
    }

    let server = Server::http(&bind_addr)?;

    eprintln!("otlp-demo-collector listening on http://{bind_addr}/v1/logs");

    for mut request in server.incoming_requests() {
        if request.method() != &Method::Post || request.url() != "/v1/logs" {
            let response = Response::from_string("not found").with_status_code(StatusCode(404));
            let _ = request.respond(response);
            continue;
        }

        let mut body = Vec::new();
        request.as_reader().read_to_end(&mut body)?;

        match ExportLogsServiceRequest::decode(body.as_slice()) {
            Ok(batch) => {
                print!("{}", format_batch_coloured(&batch));
                if delay_ms > 0 {
                    thread::sleep(Duration::from_millis(delay_ms));
                }
                let response = Response::empty(200).with_header(content_type_header());
                request.respond(response)?;
            }
            Err(err) => {
                let response = Response::from_string(format!("decode error: {err}")).with_status_code(StatusCode(400));
                request.respond(response)?;
            }
        }
    }

    Ok(())
}

fn run_tls(
    bind_addr: &str, cert_file: Option<PathBuf>, key_file: Option<PathBuf>, delay_ms: u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cert_file = cert_file.ok_or("missing --cert-file for --tls")?;
    let key_file = key_file.ok_or("missing --key-file for --tls")?;
    let config = load_demo_server_config(&cert_file, &key_file)?;
    let listener = TcpListener::bind(bind_addr)?;
    eprintln!("otlp-demo-collector listening on https://{bind_addr}/v1/logs");

    for stream in listener.incoming() {
        let stream = stream?;
        let config = config.clone();
        std::thread::spawn(move || {
            if let Err(err) = handle_tls_client(stream, config, delay_ms) {
                eprintln!("otlp-demo-collector tls client error: {err}");
            }
        });
    }

    Ok(())
}

fn handle_tls_client(
    stream: std::net::TcpStream, config: Arc<rustls::ServerConfig>, delay_ms: u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let conn = ServerConnection::new(config)?;
    let mut transport = StreamOwned::new(conn, stream);
    let result = handle_tls_http_request(&mut transport, delay_ms);
    transport.conn.send_close_notify();
    let _ = transport.flush();
    result
}

fn handle_tls_http_request(
    transport: &mut StreamOwned<ServerConnection, std::net::TcpStream>, delay_ms: u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let request = read_http_request(transport)?;
    if request.method != "POST" || request.path != "/v1/logs" {
        write_http_response(transport, 404, "not found")?;
        return Ok(());
    }

    match ExportLogsServiceRequest::decode(request.body.as_slice()) {
        Ok(batch) => {
            print!("{}", format_batch_coloured(&batch));
            if delay_ms > 0 {
                thread::sleep(Duration::from_millis(delay_ms));
            }
            write_http_response(transport, 200, "")?;
        }
        Err(err) => {
            write_http_response(transport, 400, &format!("decode error: {err}"))?;
        }
    }

    Ok(())
}

struct ParsedHttpRequest {
    method: String,
    path: String,
    body: Vec<u8>,
}

fn read_http_request<T: Read>(transport: &mut T) -> io::Result<ParsedHttpRequest> {
    let mut buffer = Vec::new();
    let mut byte = [0u8; 1];

    loop {
        transport.read_exact(&mut byte)?;
        buffer.push(byte[0]);
        if buffer.ends_with(b"\r\n\r\n") {
            break;
        }
        if buffer.len() > 16 * 1024 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "http header too large"));
        }
    }

    let header = std::str::from_utf8(&buffer[..buffer.len() - 4]).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid http header"))?;
    let mut lines = header.lines();
    let request_line = lines.next().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing request line"))?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing method"))?.to_string();
    let path = parts.next().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing path"))?.to_string();

    let mut content_length = None;
    for line in lines {
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = Some(value.trim().parse::<usize>().map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid content-length"))?);
        }
    }

    let content_length = content_length.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing content-length"))?;
    let mut body = Vec::new();
    transport.take(content_length as u64).read_to_end(&mut body)?;
    if body.len() != content_length {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "short http body"));
    }

    Ok(ParsedHttpRequest { method, path, body })
}

fn write_http_response<T: Write>(transport: &mut T, status: u16, body: &str) -> io::Result<()> {
    let status_text = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "Error",
    };
    write!(transport, "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", status, status_text, body.len(), body)?;
    transport.flush()
}
fn content_type_header() -> Header {
    Header::from_bytes(&b"Content-Type"[..], &b"application/x-protobuf"[..]).expect("static content-type header is valid")
}
