use std::env;

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use prost::Message;
use tiny_http::{Header, Method, Response, Server, StatusCode};

use otlp_demo::format_batch_colored;

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let bind_addr = env::args()
        .nth(1)
        .unwrap_or_else(|| "0.0.0.0:4318".to_string());
    let server = Server::http(&bind_addr)?;

    eprintln!("otlp-rainbow-collector listening on http://{bind_addr}/v1/logs");

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
                print!("{}", format_batch_colored(&batch));
                let response = Response::empty(200).with_header(content_type_header());
                request.respond(response)?;
            }
            Err(err) => {
                let response = Response::from_string(format!("decode error: {err}"))
                    .with_status_code(StatusCode(400));
                request.respond(response)?;
            }
        }
    }

    Ok(())
}
fn content_type_header() -> Header {
    Header::from_bytes(&b"Content-Type"[..], &b"application/x-protobuf"[..])
        .expect("static content-type header is valid")
}
