use std::env;
use std::time::Duration;

use opentelemetry_proto::tonic::collector::trace::v1::{ExportTraceServiceRequest, trace_service_client::TraceServiceClient};
use tonic::Request;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = env::args().nth(1).unwrap_or_else(|| "127.0.0.1:4317".to_string());
    let count: u64 = env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(15);
    let endpoint = if addr.starts_with("http://") || addr.starts_with("https://") { addr } else { format!("http://{addr}") };

    eprintln!("traces-grpc-emitter sending {count} OTLP trace batches to {endpoint}");

    let client = TraceServiceClient::connect(endpoint.clone()).await?;
    let mut client = client;

    for sequence in 1..=count {
        let request = otlp_demo::build_trace_request(sequence);
        match send_batch(&mut client, request).await {
            Ok(()) => eprintln!("sent OTLP gRPC trace batch #{sequence} to {endpoint}"),
            Err(err) => eprintln!("send failed for batch #{sequence}: {err}"),
        }

        if sequence < count {
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }

    eprintln!("traces-grpc-emitter finished");
    Ok(())
}

async fn send_batch(
    client: &mut TraceServiceClient<tonic::transport::Channel>, request: ExportTraceServiceRequest,
) -> Result<(), Box<dyn std::error::Error>> {
    client.export(Request::new(request)).await?;
    Ok(())
}
