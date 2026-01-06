use std::env;
use std::thread;
use std::time::Duration;

use otlp_demo::{build_excuse_request, build_message_request, format_batch_plain, post_otlp_http};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let mut addr = "127.0.0.1:4318".to_string();
    let mut count = None;
    let mut interval_ms = 1_000u64;
    let mut once_message = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--count" => {
                count = Some(
                    args.next()
                        .ok_or("missing value for --count")?
                        .parse::<u64>()?,
                );
            }
            "--interval-ms" => {
                interval_ms = args
                    .next()
                    .ok_or("missing value for --interval-ms")?
                    .parse::<u64>()?;
            }
            "--once" => {
                count = Some(1);
            }
            "--message" => {
                once_message = Some(args.next().ok_or("missing value for --message")?);
            }
            value if value.starts_with("--") => return Err(format!("unknown argument: {value}").into()),
            value => addr = value.to_string(),
        }
    }

    eprintln!("otlp-bofh-emitter sending OTLP logs to http://{addr}/v1/logs");

    let mut sequence = 1u64;
    loop {
        let request = match &once_message {
            Some(message) => build_message_request(sequence, message.clone()),
            None => build_excuse_request(sequence),
        };
        print!("{}", format_batch_plain(&request));
        match post_otlp_http(&addr, &request) {
            Ok(()) => eprintln!("sent OTLP log batch #{sequence} to http://{addr}/v1/logs"),
            Err(err) => eprintln!("send failed for batch #{sequence}: {err}"),
        }

        sequence += 1;
        if let Some(max_count) = count {
            if sequence > max_count {
                break;
            }
        }

        if interval_ms > 0 {
            thread::sleep(Duration::from_millis(interval_ms));
        }
    }

    Ok(())
}
