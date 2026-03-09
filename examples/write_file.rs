use std::fs::File;
use std::io::BufWriter;

use logjet::{LogjetWriter, RecordType};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let file = File::create("telemetry.logjet")?;
    let writer = BufWriter::new(file);
    let mut log_writer = LogjetWriter::new(writer);

    log_writer.push(
        RecordType::Logs,
        1,
        1_700_000_000_000_000_000,
        b"fake-otlp-logs",
    )?;
    log_writer.push(
        RecordType::Metrics,
        2,
        1_700_000_000_000_000_100,
        b"fake-otlp-metrics",
    )?;
    log_writer.push(
        RecordType::Traces,
        3,
        1_700_000_000_000_000_200,
        b"fake-otlp-traces",
    )?;

    let mut writer = log_writer.into_inner()?;
    use std::io::Write;
    writer.flush()?;
    Ok(())
}
