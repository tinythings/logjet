use std::env;
use std::io::{self, ErrorKind, Read, Write};
use std::net::TcpStream;

use logjet::RecordType;
use otlp_demo::post_raw_otlp_http;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let source = args.next().unwrap_or_else(|| "127.0.0.1:7002".to_string());
    let dest = args.next().unwrap_or_else(|| "127.0.0.1:4320".to_string());
    let max_records = match args.next() {
        Some(value) => Some(value.parse::<u64>()?),
        None => None,
    };

    let mut stream = TcpStream::connect(&source)?;
    read_replay_hello(&mut stream)?;
    write_replay_request(&mut stream, 0)?;
    let mut forwarded = 0u64;

    while let Some(record) = read_wire_record(&mut stream)? {
        if record.record_type == RecordType::Logs {
            post_raw_otlp_http(&dest, &record.payload, None, None)?;
            forwarded += 1;
            eprintln!(
                "forwarded record seq={} to http://{dest}/v1/logs",
                record.seq
            );
        }

        if let Some(limit) = max_records {
            if forwarded >= limit {
                break;
            }
        }
    }

    eprintln!("forwarded {forwarded} record(s)");
    Ok(())
}

fn write_replay_request(stream: &mut TcpStream, from_seq: u64) -> io::Result<()> {
    stream.write_all(b"LJRPL001")?;
    stream.write_all(&[1])?;
    stream.write_all(&[0u8; 7])?;
    stream.write_all(&from_seq.to_le_bytes())?;
    Ok(())
}

fn read_replay_hello(stream: &mut TcpStream) -> io::Result<()> {
    let mut magic = [0u8; 8];
    stream.read_exact(&mut magic)?;
    if magic != *b"LJRPH001" {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "invalid replay hello magic",
        ));
    }

    let mut header = [0u8; 32];
    stream.read_exact(&mut header)?;
    if header[0] != 1 {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("unsupported replay hello version: {}", header[0]),
        ));
    }

    Ok(())
}

struct WireRecord {
    record_type: RecordType,
    seq: u64,
    payload: Vec<u8>,
}

fn read_wire_record<R: Read>(reader: &mut R) -> io::Result<Option<WireRecord>> {
    let mut magic = [0u8; 8];
    match reader.read_exact(&mut magic) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err),
    }

    if magic != *b"LJNETV01" {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "invalid wire protocol magic",
        ));
    }

    let mut header = [0u8; 24];
    reader.read_exact(&mut header)?;
    let record_type = RecordType::from_u8(header[1])
        .map_err(|err| io::Error::new(ErrorKind::InvalidData, err.to_string()))?;
    let payload_len = u32::from_le_bytes([header[20], header[21], header[22], header[23]]) as usize;
    let mut payload = vec![0u8; payload_len];
    reader.read_exact(&mut payload)?;

    Ok(Some(WireRecord {
        record_type,
        seq: u64::from_le_bytes([
            header[4], header[5], header[6], header[7], header[8], header[9], header[10],
            header[11],
        ]),
        payload,
    }))
}
