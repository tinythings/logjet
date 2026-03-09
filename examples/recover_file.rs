use std::fs::File;
use std::io::BufReader;

use logjet::LogjetReader;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let file = File::open("telemetry.logjet")?;
    let mut reader = LogjetReader::new(BufReader::new(file));

    while let Some(record) = reader.next_record()? {
        println!("recovered {:?} seq={} payload_len={}", record.record_type, record.seq, record.payload.len());
    }

    let stats = reader.stats();
    println!("blocks_ok={} blocks_bad={} bytes_skipped={} records_ok={}", stats.blocks_ok, stats.blocks_bad, stats.bytes_skipped, stats.records_ok);
    Ok(())
}
