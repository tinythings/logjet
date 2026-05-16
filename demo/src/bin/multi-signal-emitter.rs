use std::env;
use std::process;
use std::thread;
use std::time::Duration;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: multi-signal-emitter <addr> [count]");
        eprintln!("  addr  - host:port of the OTLP/HTTP endpoint (e.g. 127.0.0.1:4318)");
        eprintln!("  count - number of batches per signal to emit (default: 8)");
        process::exit(1);
    }

    let addr = &args[1];
    let count: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(8);

    let logs_endpoint = if addr.starts_with("http://") || addr.starts_with("https://") {
        format!("{addr}/v1/logs")
    } else {
        format!("http://{addr}/v1/logs")
    };
    let metrics_endpoint = if addr.starts_with("http://") || addr.starts_with("https://") {
        format!("{addr}/v1/metrics")
    } else {
        format!("http://{addr}/v1/metrics")
    };
    let traces_endpoint = if addr.starts_with("http://") || addr.starts_with("https://") {
        format!("{addr}/v1/traces")
    } else {
        format!("http://{addr}/v1/traces")
    };

    println!("multi-signal-emitter sending {count} batches per signal (logs, metrics, traces) to {addr}");

    for sequence in 1..=count {
        let log_request = otlp_demo::build_excuse_request(sequence);
        match otlp_demo::post_otlp_http(&logs_endpoint, &log_request) {
            Ok(()) => println!("seq={sequence} -> sent logs batch"),
            Err(err) => eprintln!("seq={sequence} -> logs error: {err}"),
        }

        thread::sleep(Duration::from_millis(200));

        let metric_request = otlp_demo::build_metrics_request(sequence);
        match otlp_demo::post_otlp_http_metrics(&metrics_endpoint, &metric_request) {
            Ok(()) => println!("seq={sequence} -> sent metrics batch"),
            Err(err) => eprintln!("seq={sequence} -> metrics error: {err}"),
        }

        thread::sleep(Duration::from_millis(200));

        let trace_request = otlp_demo::build_trace_request(sequence);
        match otlp_demo::post_otlp_http_traces(&traces_endpoint, &trace_request) {
            Ok(()) => println!("seq={sequence} -> sent traces batch"),
            Err(err) => eprintln!("seq={sequence} -> traces error: {err}"),
        }

        if sequence < count {
            thread::sleep(Duration::from_millis(500));
        }
    }

    println!("multi-signal-emitter finished");
}
