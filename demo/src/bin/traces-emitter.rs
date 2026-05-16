use std::env;
use std::process;
use std::thread;
use std::time::Duration;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: traces-emitter <addr> [count]");
        eprintln!("  addr  - host:port of the OTLP/HTTP traces endpoint (e.g. 127.0.0.1:4318)");
        eprintln!("  count - number of trace batches to emit (default: 15)");
        process::exit(1);
    }

    let addr = &args[1];
    let count: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(15);

    let endpoint = if addr.starts_with("http://") || addr.starts_with("https://") {
        format!("{addr}/v1/traces")
    } else {
        format!("http://{addr}/v1/traces")
    };

    println!("traces-emitter sending {count} batches to {endpoint}");

    for sequence in 1..=count {
        let request = otlp_demo::build_trace_request(sequence);
        match otlp_demo::post_otlp_http_traces(&endpoint, &request) {
            Ok(()) => {
                println!("seq={sequence} -> sent traces batch");
            }
            Err(err) => {
                eprintln!("seq={sequence} -> error: {err}");
            }
        }
        if sequence < count {
            thread::sleep(Duration::from_millis(300));
        }
    }

    println!("traces-emitter finished");
}
