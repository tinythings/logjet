use std::fs::File;
use std::io::BufReader;

use logjet::LogjetReader;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let file = File::open("telemetry.logjet")?;
    let mut reader = LogjetReader::new(BufReader::new(file));

    while let Some(record) = reader.next_record()? {
        println!("type={:?} seq={} ts={} payload_len={}", record.record_type, record.seq, record.ts_unix_ns, record.payload.len());
    }

    println!("stats={:?}", reader.stats());
    Ok(())
}
