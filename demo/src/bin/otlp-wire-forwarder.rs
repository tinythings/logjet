use std::env;
use std::io::{self, ErrorKind, Read, Write};
use std::net::TcpStream;

use logjet::RecordType;
use otlp_demo::DemoConnection;

const WIRE_MAGIC: [u8; 8] = *b"LJNETV01";
const WIRE_VERSION: u8 = 1;

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
    let mut conn = DemoConnection::open(&dest, None, None)?;
    let mut forwarded = 0u64;

    while let Some(record) = read_wire_record(&mut stream)? {
        if record.record_type == RecordType::Logs {
            conn.post(&record.payload)?;
            forwarded += 1;
            eprintln!("forwarded record seq={} to http://{dest}/v1/logs", record.seq);
        }

        if let Some(limit) = max_records
            && forwarded >= limit
        {
            break;
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
        return Err(io::Error::new(ErrorKind::InvalidData, "invalid replay hello magic"));
    }

    let mut header = [0u8; 32];
    stream.read_exact(&mut header)?;
    if header[0] != 1 {
        return Err(io::Error::new(ErrorKind::InvalidData, format!("unsupported replay hello version: {}", header[0])));
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

    if magic != WIRE_MAGIC {
        return Err(io::Error::new(ErrorKind::InvalidData, "invalid wire protocol magic"));
    }

    let mut header = [0u8; 24];
    reader.read_exact(&mut header)?;
    if header[0] != WIRE_VERSION {
        return Err(io::Error::new(ErrorKind::InvalidData, format!("unsupported wire protocol version: {}", header[0])));
    }

    let record_type = RecordType::from_u8(header[1]).map_err(|err| io::Error::new(ErrorKind::InvalidData, err.to_string()))?;
    let codec = header[2];
    let payload_len = u32::from_le_bytes([header[20], header[21], header[22], header[23]]) as usize;
    let mut wire_payload = vec![0u8; payload_len];
    reader.read_exact(&mut wire_payload)?;

    let mut crc = [0u8; 4];
    reader.read_exact(&mut crc)?;

    let mut crc_input = Vec::with_capacity(header.len() + wire_payload.len());
    crc_input.extend_from_slice(&header);
    crc_input.extend_from_slice(&wire_payload);
    let actual_crc = logjet::crc::crc32c(&crc_input);
    let expected_crc = u32::from_le_bytes(crc);
    if actual_crc != expected_crc {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("wire record CRC32C mismatch: expected {expected_crc:#010x}, got {actual_crc:#010x}"),
        ));
    }

    let payload = match codec {
        0 => wire_payload,
        1 => {
            if wire_payload.len() < 4 {
                return Err(io::Error::new(ErrorKind::InvalidData, "LZ4 wire payload too short for uncompressed length"));
            }
            let uncompressed_len = u32::from_le_bytes([wire_payload[0], wire_payload[1], wire_payload[2], wire_payload[3]]) as usize;
            lz4_flex::block::decompress(&wire_payload[4..], uncompressed_len)
                .map_err(|err| io::Error::new(ErrorKind::InvalidData, format!("LZ4 decompress failed: {err}")))?
        }
        other => return Err(io::Error::new(ErrorKind::InvalidData, format!("unknown wire codec: {other}"))),
    };

    Ok(Some(WireRecord {
        record_type,
        seq: u64::from_le_bytes([header[4], header[5], header[6], header[7], header[8], header[9], header[10], header[11]]),
        payload,
    }))
}
