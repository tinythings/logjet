use std::env;
use std::io::Write;
use std::net::TcpStream;
use std::thread;
use std::time::Duration;

use logjet::RecordType;

const WIRE_MAGIC: [u8; 8] = *b"LJNETV01";
const WIRE_VERSION: u8 = 1;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let mut addr = "127.0.0.1:7001".to_string();
    let mut hold_ms = 4_000u64;
    let mut service_name = "wire-holder".to_string();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--hold-ms" => {
                hold_ms = args
                    .next()
                    .ok_or("missing value for --hold-ms")?
                    .parse::<u64>()?;
            }
            "--service-name" => {
                service_name = args.next().ok_or("missing value for --service-name")?;
            }
            value if value.starts_with("--") => {
                return Err(format!("unknown argument: {value}").into());
            }
            value => addr = value.to_string(),
        }
    }

    eprintln!("wire-hold-emitter connecting to {addr} as {service_name}");
    let mut stream = TcpStream::connect(&addr)?;
    stream.set_nodelay(true)?;

    let first_message = format!("{service_name} first record: holding ingest slot");
    write_wire_record(&mut stream, 1, first_message.as_bytes())?;
    eprintln!("{service_name}: first record sent, holding for {hold_ms} ms");

    thread::sleep(Duration::from_millis(hold_ms));

    let second_message = format!("{service_name} second record: connection still alive");
    match write_wire_record(&mut stream, 2, second_message.as_bytes()) {
        Ok(()) => {
            eprintln!("{service_name}: second record sent; connection stayed open");
            Ok(())
        }
        Err(err) => {
            eprintln!("{service_name}: second record failed; connection was closed: {err}");
            Err(err.into())
        }
    }
}

fn write_wire_record(
    writer: &mut TcpStream,
    seq: u64,
    payload: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let payload_len = u32::try_from(payload.len())?;
    writer.write_all(&WIRE_MAGIC)?;
    writer.write_all(&[WIRE_VERSION, RecordType::Logs as u8])?;
    writer.write_all(&0u16.to_le_bytes())?;
    writer.write_all(&seq.to_le_bytes())?;
    writer.write_all(&0u64.to_le_bytes())?;
    writer.write_all(&payload_len.to_le_bytes())?;
    writer.write_all(payload)?;
    writer.flush()?;
    Ok(())
}
