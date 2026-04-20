use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use logjet::{LogjetWriter, RecordType, WriterConfig};
use otlp_demo::build_excuse_request;
use prost::Message;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let output = match args.next() {
        Some(path) => PathBuf::from(path),
        None => {
            eprintln!("usage: otlp-bofh-logjet-generator <output.logjet> [count]");
            std::process::exit(2);
        }
    };
    let count = args.next().map(|value| value.parse::<u64>()).transpose()?.unwrap_or(5_000);

    let file = File::create(&output)?;
    let writer = BufWriter::new(file);
    let mut logjet = LogjetWriter::with_config(writer, WriterConfig::default());

    for seq in 1..=count {
        let request = build_excuse_request(seq);
        logjet.push(RecordType::Logs, seq, unix_time_nanos(seq), &request.encode_to_vec())?;
    }

    let mut writer = logjet.into_inner()?;
    writer.flush()?;
    println!("wrote {count} BOFH log records to {}", output.display());
    Ok(())
}

fn unix_time_nanos(seq: u64) -> u64 {
    let base = 1_773_000_000_000_000_000u64;
    base.saturating_add(seq.saturating_mul(1_000_000))
}
