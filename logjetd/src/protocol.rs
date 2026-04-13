use std::io::{self, ErrorKind, Read, Write};

use logjet::RecordType;

pub const WIRE_MAGIC: [u8; 8] = *b"LJNETV01";
pub const WIRE_VERSION: u8 = 1;
pub const REPLAY_REQUEST_MAGIC: [u8; 8] = *b"LJRPL001";
pub const REPLAY_REQUEST_VERSION: u8 = 1;
pub const REPLAY_HELLO_MAGIC: [u8; 8] = *b"LJRPH001";
pub const REPLAY_HELLO_VERSION: u8 = 1;
pub const REPLAY_ACK_MAGIC: [u8; 8] = *b"LJRPA001";
pub const REPLAY_ACK_VERSION: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireRecord {
    pub record_type: RecordType,
    pub seq: u64,
    pub ts_unix_ns: u64,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayRequest {
    pub from_seq: u64,
    pub consume: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayAck {
    pub ack_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayHello {
    pub stream_id: u64,
    pub first_seq: u64,
    pub last_seq: u64,
}

pub fn read_record<R: Read>(reader: &mut R) -> io::Result<Option<WireRecord>> {
    read_record_with_limit(reader, usize::MAX)
}

pub fn read_record_with_limit<R: Read>(reader: &mut R, max_payload_len: usize) -> io::Result<Option<WireRecord>> {
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

    let version = header[0];
    if version != WIRE_VERSION {
        return Err(io::Error::new(ErrorKind::InvalidData, format!("unsupported wire protocol version: {version}")));
    }

    let record_type =
        RecordType::from_u8(header[1]).map_err(|err| io::Error::new(ErrorKind::InvalidData, format!("invalid wire record type: {err}")))?;
    let codec = header[2];
    let payload_len = u32::from_le_bytes([header[20], header[21], header[22], header[23]]) as usize;
    if payload_len > max_payload_len {
        return Err(io::Error::new(ErrorKind::InvalidData, format!("wire payload too large: {payload_len} > {max_payload_len}")));
    }

    let mut wire_payload = vec![0u8; payload_len];
    reader.read_exact(&mut wire_payload)?;

    let mut crc_bytes = [0u8; 4];
    reader.read_exact(&mut crc_bytes)?;
    let expected_crc = u32::from_le_bytes(crc_bytes);

    let mut crc_input = Vec::with_capacity(header.len() + wire_payload.len());
    crc_input.extend_from_slice(&header);
    crc_input.extend_from_slice(&wire_payload);
    let actual_crc = logjet::crc::crc32c(&crc_input);

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
        ts_unix_ns: u64::from_le_bytes([header[12], header[13], header[14], header[15], header[16], header[17], header[18], header[19]]),
        payload,
    }))
}

pub fn write_record<W: Write>(writer: &mut W, record: &WireRecord, compress: bool) -> io::Result<()> {
    let (codec, wire_payload) = if compress {
        let compressed = lz4_flex::block::compress(&record.payload);

        if compressed.len() < record.payload.len() {
            let uncompressed_len =
                u32::try_from(record.payload.len()).map_err(|_| io::Error::new(ErrorKind::InvalidInput, "payload too large for wire protocol"))?;

            let mut lz4_payload = Vec::with_capacity(4 + compressed.len());

            lz4_payload.extend_from_slice(&uncompressed_len.to_le_bytes());

            lz4_payload.extend_from_slice(&compressed);

            (1u8, lz4_payload)
        } else {
            (0u8, record.payload.clone())
        }
    } else {
        (0u8, record.payload.clone())
    };

    let payload_len =
        u32::try_from(wire_payload.len()).map_err(|_| io::Error::new(ErrorKind::InvalidInput, "payload too large for wire protocol"))?;

    let mut buf = Vec::with_capacity(8 + 24 + wire_payload.len() + 4);
    buf.extend_from_slice(&WIRE_MAGIC);
    buf.push(WIRE_VERSION);
    buf.push(record.record_type as u8);
    buf.push(codec);
    buf.push(0);
    buf.extend_from_slice(&record.seq.to_le_bytes());
    buf.extend_from_slice(&record.ts_unix_ns.to_le_bytes());
    buf.extend_from_slice(&payload_len.to_le_bytes());
    buf.extend_from_slice(&wire_payload);

    let crc = logjet::crc::crc32c(&buf[WIRE_MAGIC.len()..]);
    buf.extend_from_slice(&crc.to_le_bytes());

    writer.write_all(&buf)
}

pub fn read_replay_request<R: Read>(reader: &mut R) -> io::Result<ReplayRequest> {
    let mut magic = [0u8; 8];
    reader.read_exact(&mut magic)?;
    if magic != REPLAY_REQUEST_MAGIC {
        return Err(io::Error::new(ErrorKind::InvalidData, "invalid replay request magic"));
    }

    let mut header = [0u8; 16];
    reader.read_exact(&mut header)?;
    let version = header[0];
    if version != REPLAY_REQUEST_VERSION {
        return Err(io::Error::new(ErrorKind::InvalidData, format!("unsupported replay request version: {version}")));
    }

    Ok(ReplayRequest {
        consume: header[1] & 0x01 != 0,
        from_seq: u64::from_le_bytes([header[8], header[9], header[10], header[11], header[12], header[13], header[14], header[15]]),
    })
}

pub fn write_replay_request<W: Write>(writer: &mut W, request: &ReplayRequest) -> io::Result<()> {
    let mut buf = [0u8; 24];
    buf[..8].copy_from_slice(&REPLAY_REQUEST_MAGIC);
    buf[8] = REPLAY_REQUEST_VERSION;
    buf[9] = u8::from(request.consume);
    buf[16..24].copy_from_slice(&request.from_seq.to_le_bytes());
    writer.write_all(&buf)
}

pub fn read_replay_hello<R: Read>(reader: &mut R) -> io::Result<ReplayHello> {
    let mut magic = [0u8; 8];
    reader.read_exact(&mut magic)?;
    if magic != REPLAY_HELLO_MAGIC {
        return Err(io::Error::new(ErrorKind::InvalidData, "invalid replay hello magic"));
    }

    let mut header = [0u8; 32];
    reader.read_exact(&mut header)?;
    let version = header[0];
    if version != REPLAY_HELLO_VERSION {
        return Err(io::Error::new(ErrorKind::InvalidData, format!("unsupported replay hello version: {version}")));
    }

    Ok(ReplayHello {
        stream_id: u64::from_le_bytes([header[8], header[9], header[10], header[11], header[12], header[13], header[14], header[15]]),
        first_seq: u64::from_le_bytes([header[16], header[17], header[18], header[19], header[20], header[21], header[22], header[23]]),
        last_seq: u64::from_le_bytes([header[24], header[25], header[26], header[27], header[28], header[29], header[30], header[31]]),
    })
}

pub fn write_replay_hello<W: Write>(writer: &mut W, hello: &ReplayHello) -> io::Result<()> {
    let mut buf = [0u8; 40];
    buf[..8].copy_from_slice(&REPLAY_HELLO_MAGIC);
    buf[8] = REPLAY_HELLO_VERSION;
    buf[16..24].copy_from_slice(&hello.stream_id.to_le_bytes());
    buf[24..32].copy_from_slice(&hello.first_seq.to_le_bytes());
    buf[32..40].copy_from_slice(&hello.last_seq.to_le_bytes());
    writer.write_all(&buf)
}

pub fn read_replay_ack<R: Read>(reader: &mut R) -> io::Result<ReplayAck> {
    let mut magic = [0u8; 8];
    reader.read_exact(&mut magic)?;
    if magic != REPLAY_ACK_MAGIC {
        return Err(io::Error::new(ErrorKind::InvalidData, "invalid replay ack magic"));
    }

    let mut header = [0u8; 16];
    reader.read_exact(&mut header)?;
    let version = header[0];
    if version != REPLAY_ACK_VERSION {
        return Err(io::Error::new(ErrorKind::InvalidData, format!("unsupported replay ack version: {version}")));
    }

    Ok(ReplayAck { ack_seq: u64::from_le_bytes([header[8], header[9], header[10], header[11], header[12], header[13], header[14], header[15]]) })
}

pub fn write_replay_ack<W: Write>(writer: &mut W, ack: &ReplayAck) -> io::Result<()> {
    let mut buf = [0u8; 24];
    buf[..8].copy_from_slice(&REPLAY_ACK_MAGIC);
    buf[8] = REPLAY_ACK_VERSION;
    buf[16..24].copy_from_slice(&ack.ack_seq.to_le_bytes());
    writer.write_all(&buf)
}

#[cfg(test)]
#[path = "protocol_utst.rs"]
mod protocol_utst;
