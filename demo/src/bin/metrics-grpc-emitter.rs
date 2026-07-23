use std::env;
use std::time::Duration;

use opentelemetry_proto::tonic::collector::metrics::v1::{ExportMetricsServiceRequest, metrics_service_client::MetricsServiceClient};
use tonic::Request;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = env::args().nth(1).unwrap_or_else(|| "127.0.0.1:4317".to_string());
    let count: u64 = env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(15);
    let endpoint = if addr.starts_with("http://") || addr.starts_with("https://") { addr } else { format!("http://{addr}") };

    eprintln!("metrics-grpc-emitter sending {count} OTLP metrics batches to {endpoint}");

    let client = MetricsServiceClient::connect(endpoint.clone()).await?;
    let mut client = client;

    for sequence in 1..=count {
        let request = otlp_demo::build_metrics_request(sequence);
        match send_batch(&mut client, request).await {
            Ok(()) => eprintln!("sent OTLP gRPC metrics batch #{sequence} to {endpoint}"),
            Err(err) => eprintln!("send failed for batch #{sequence}: {err}"),
        }

        if sequence < count {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    eprintln!("metrics-grpc-emitter finished");
    Ok(())
}

async fn send_batch(
    client: &mut MetricsServiceClient<tonic::transport::Channel>, request: ExportMetricsServiceRequest,
) -> Result<(), Box<dyn std::error::Error>> {
    client.export(Request::new(request)).await?;
    Ok(())
}
