use std::env;
use std::process;
use std::thread;
use std::time::Duration;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: metrics-emitter <addr> [count]");
        eprintln!("  addr  - host:port of the OTLP/HTTP metrics endpoint (e.g. 127.0.0.1:4318)");
        eprintln!("  count - number of metric batches to emit (default: 20)");
        process::exit(1);
    }

    let addr = &args[1];
    let count: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(20);

    let endpoint =
        if addr.starts_with("http://") || addr.starts_with("https://") { format!("{addr}/v1/metrics") } else { format!("http://{addr}/v1/metrics") };

    println!("metrics-emitter sending {count} batches to {endpoint}");

    for sequence in 1..=count {
        let request = otlp_demo::build_metrics_request(sequence);
        match otlp_demo::post_otlp_http_metrics(&endpoint, &request) {
            Ok(()) => {
                println!("seq={sequence} -> sent metrics batch");
            }
            Err(err) => {
                eprintln!("seq={sequence} -> error: {err}");
            }
        }
        if sequence < count {
            thread::sleep(Duration::from_millis(200));
        }
    }

    println!("metrics-emitter finished");
}
