use std::env;
use std::thread;
use std::time::Duration;

use otlp_demo::{build_excuse_request, format_batch_plain, post_otlp_http};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:4318".to_string());

    eprintln!("otlp-bofh-emitter sending OTLP logs to http://{addr}/v1/logs");

    let mut sequence = 1u64;
    loop {
        let request = build_excuse_request(sequence);
        print!("{}", format_batch_plain(&request));
        match post_otlp_http(&addr, &request) {
            Ok(()) => eprintln!("sent OTLP log batch #{sequence} to http://{addr}/v1/logs"),
            Err(err) => eprintln!("send failed for batch #{sequence}: {err}"),
        }

        sequence += 1;
        thread::sleep(Duration::from_secs(1));
    }
}
