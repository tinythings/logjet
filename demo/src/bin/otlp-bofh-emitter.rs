use std::env;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use prost::Message;

use otlp_demo::{build_excuse_request, build_excuse_request_for_service_with_severity, build_message_request_for_service, format_batch_plain};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let mut addr = "127.0.0.1:4318".to_string();
    let mut count = None;
    let mut interval_ms = 1_000u64;
    let mut once_message = None;
    let mut service_name = "bofh-emitter".to_string();
    let mut severity = "warn".to_string();
    let mut ca_file = None;
    let mut server_name = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--count" => {
                count = Some(args.next().ok_or("missing value for --count")?.parse::<u64>()?);
            }
            "--interval-ms" => {
                interval_ms = args.next().ok_or("missing value for --interval-ms")?.parse::<u64>()?;
            }
            "--once" => {
                count = Some(1);
            }
            "--message" => {
                once_message = Some(args.next().ok_or("missing value for --message")?);
            }
            "--service-name" => {
                service_name = args.next().ok_or("missing value for --service-name")?;
            }
            "--severity" => {
                severity = args.next().ok_or("missing value for --severity")?;
            }
            "--ca-file" => {
                ca_file = Some(PathBuf::from(args.next().ok_or("missing value for --ca-file")?));
            }
            "--server-name" => {
                server_name = Some(args.next().ok_or("missing value for --server-name")?);
            }
            value if value.starts_with("--") => {
                return Err(format!("unknown argument: {value}").into());
            }
            value => addr = value.to_string(),
        }
    }

    let display_target = if addr.starts_with("http://") || addr.starts_with("https://") {
        format!("{addr}/v1/logs").replace("/v1/logs/v1/logs", "/v1/logs")
    } else {
        format!("http://{addr}/v1/logs")
    };
    eprintln!("otlp-bofh-emitter sending OTLP logs to {display_target}");

    let mut sequence = 1u64;
    loop {
        let request = match &once_message {
            Some(message) => build_message_request_for_service(sequence, &service_name, &severity, message.clone()),
            None if service_name == "bofh-emitter" && severity == "warn" => build_excuse_request(sequence),
            None => build_excuse_request_for_service_with_severity(sequence, &service_name, &severity),
        };
        print!("{}", format_batch_plain(&request));
        match otlp_demo::post_raw_otlp_http(&addr, &request.encode_to_vec(), ca_file.as_deref(), server_name.as_deref()) {
            Ok(()) => eprintln!("sent OTLP log batch #{sequence} to {display_target}"),
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
