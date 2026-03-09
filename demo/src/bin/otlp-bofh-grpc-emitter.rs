use std::env;
use std::time::Duration;

use opentelemetry_proto::tonic::collector::logs::v1::{
    ExportLogsServiceRequest, logs_service_client::LogsServiceClient,
};
use tonic::Request;

use otlp_demo::{build_excuse_request, format_batch_plain};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:4317".to_string());
    let endpoint = if addr.starts_with("http://") || addr.starts_with("https://") {
        addr
    } else {
        format!("http://{addr}")
    };

    eprintln!("otlp-bofh-grpc-emitter sending OTLP logs to {endpoint}");

    let mut sequence = 1u64;
    loop {
        let request = build_excuse_request(sequence);
        print!("{}", format_batch_plain(&request));
        match send_batch(&endpoint, request).await {
            Ok(()) => eprintln!("sent OTLP gRPC log batch #{sequence} to {endpoint}"),
            Err(err) => eprintln!("send failed for batch #{sequence}: {err}"),
        }

        sequence += 1;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn send_batch(
    endpoint: &str,
    request: ExportLogsServiceRequest,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut client = LogsServiceClient::connect(endpoint.to_string()).await?;
    client.export(Request::new(request)).await?;
    Ok(())
}
