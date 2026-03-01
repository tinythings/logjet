use std::env;
use std::io::{self, ErrorKind, Read, Write};
use std::net::TcpStream;
use std::thread;
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let source = args
        .next()
        .unwrap_or_else(|| "127.0.0.1:7002".to_string());
    let stall_ms = match args.next() {
        Some(value) => value.parse::<u64>()?,
        None => 10_000,
    };

    let mut stream = TcpStream::connect(&source)?;
    let hello = read_replay_hello(&mut stream)?;
    eprintln!(
        "stall client connected to {source}; stream_id={} first_seq={} last_seq={}",
        hello.stream_id, hello.first_seq, hello.last_seq
    );
    write_replay_request(&mut stream, 0, true)?;

    let Some(record) = read_wire_record(&mut stream)? else {
        eprintln!("stall client received no record");
        return Ok(());
    };

    eprintln!(
        "stall client received seq={} and will now stop acknowledging for {} ms",
        record.seq, stall_ms
    );
    thread::sleep(Duration::from_millis(stall_ms));

    let mut extra = [0u8; 1];
    match stream.read(&mut extra) {
        Ok(0) => eprintln!("stall client connection was closed as expected"),
        Ok(_) => eprintln!("stall client unexpectedly received extra data"),
        Err(err) => eprintln!("stall client read after stall ended with: {err}"),
    }

    Ok(())
}

struct ReplayHello {
    stream_id: u64,
    first_seq: u64,
    last_seq: u64,
}

fn write_replay_request(stream: &mut TcpStream, from_seq: u64, consume: bool) -> io::Result<()> {
    stream.write_all(b"LJRPL001")?;
    stream.write_all(&[1])?;
    stream.write_all(&[u8::from(consume)])?;
    stream.write_all(&[0u8; 6])?;
    stream.write_all(&from_seq.to_le_bytes())?;
    stream.flush()
}

fn read_replay_hello(stream: &mut TcpStream) -> io::Result<ReplayHello> {
    let mut magic = [0u8; 8];
    stream.read_exact(&mut magic)?;
    if magic != *b"LJRPH001" {
        return Err(io::Error::new(ErrorKind::InvalidData, "invalid replay hello magic"));
    }

    let mut header = [0u8; 32];
    stream.read_exact(&mut header)?;
    if header[0] != 1 {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("unsupported replay hello version: {}", header[0]),
        ));
    }

    Ok(ReplayHello {
        stream_id: u64::from_le_bytes([
            header[8], header[9], header[10], header[11], header[12], header[13], header[14], header[15],
        ]),
        first_seq: u64::from_le_bytes([
            header[16], header[17], header[18], header[19], header[20], header[21], header[22], header[23],
        ]),
        last_seq: u64::from_le_bytes([
            header[24], header[25], header[26], header[27], header[28], header[29], header[30], header[31],
        ]),
    })
}

struct WireRecord {
    seq: u64,
}

fn read_wire_record<R: Read>(reader: &mut R) -> io::Result<Option<WireRecord>> {
    let mut magic = [0u8; 8];
    match reader.read_exact(&mut magic) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err),
    }

    if magic != *b"LJNETV01" {
        return Err(io::Error::new(ErrorKind::InvalidData, "invalid wire protocol magic"));
    }

    let mut header = [0u8; 24];
    reader.read_exact(&mut header)?;
    let payload_len =
        u32::from_le_bytes([header[20], header[21], header[22], header[23]]) as usize;
    let mut payload = vec![0u8; payload_len];
    reader.read_exact(&mut payload)?;

    Ok(Some(WireRecord {
        seq: u64::from_le_bytes([
            header[4], header[5], header[6], header[7], header[8], header[9], header[10], header[11],
        ]),
    }))
}
