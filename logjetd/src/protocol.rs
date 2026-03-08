use std::io::{self, ErrorKind, Read, Write};

use logjet::RecordType;

pub const WIRE_MAGIC: [u8; 8] = *b"LJNETV01";
pub const WIRE_VERSION: u8 = 1;
pub const REPLAY_REQUEST_MAGIC: [u8; 8] = *b"LJRPL001";
pub const REPLAY_REQUEST_VERSION: u8 = 1;

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
}

pub fn read_record<R: Read>(reader: &mut R) -> io::Result<Option<WireRecord>> {
    let mut magic = [0u8; 8];
    match reader.read_exact(&mut magic) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err),
    }

    if magic != WIRE_MAGIC {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "invalid wire protocol magic",
        ));
    }

    let mut header = [0u8; 24];
    reader.read_exact(&mut header)?;

    let version = header[0];
    if version != WIRE_VERSION {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("unsupported wire protocol version: {version}"),
        ));
    }

    let record_type = RecordType::from_u8(header[1]).map_err(|err| {
        io::Error::new(
            ErrorKind::InvalidData,
            format!("invalid wire record type: {err}"),
        )
    })?;
    let payload_len = u32::from_le_bytes([header[20], header[21], header[22], header[23]]) as usize;

    let mut payload = vec![0u8; payload_len];
    reader.read_exact(&mut payload)?;

    Ok(Some(WireRecord {
        record_type,
        seq: u64::from_le_bytes([
            header[4], header[5], header[6], header[7], header[8], header[9], header[10], header[11],
        ]),
        ts_unix_ns: u64::from_le_bytes([
            header[12], header[13], header[14], header[15], header[16], header[17], header[18], header[19],
        ]),
        payload,
    }))
}

pub fn write_record<W: Write>(writer: &mut W, record: &WireRecord) -> io::Result<()> {
    let payload_len = u32::try_from(record.payload.len())
        .map_err(|_| io::Error::new(ErrorKind::InvalidInput, "payload too large for wire protocol"))?;

    writer.write_all(&WIRE_MAGIC)?;
    writer.write_all(&[WIRE_VERSION, record.record_type as u8])?;
    writer.write_all(&0u16.to_le_bytes())?;
    writer.write_all(&record.seq.to_le_bytes())?;
    writer.write_all(&record.ts_unix_ns.to_le_bytes())?;
    writer.write_all(&payload_len.to_le_bytes())?;
    writer.write_all(&record.payload)?;
    Ok(())
}

pub fn read_replay_request<R: Read>(reader: &mut R) -> io::Result<ReplayRequest> {
    let mut magic = [0u8; 8];
    reader.read_exact(&mut magic)?;
    if magic != REPLAY_REQUEST_MAGIC {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "invalid replay request magic",
        ));
    }

    let mut header = [0u8; 16];
    reader.read_exact(&mut header)?;
    let version = header[0];
    if version != REPLAY_REQUEST_VERSION {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("unsupported replay request version: {version}"),
        ));
    }

    Ok(ReplayRequest {
        from_seq: u64::from_le_bytes([
            header[8], header[9], header[10], header[11], header[12], header[13], header[14], header[15],
        ]),
    })
}

pub fn write_replay_request<W: Write>(writer: &mut W, request: &ReplayRequest) -> io::Result<()> {
    writer.write_all(&REPLAY_REQUEST_MAGIC)?;
    writer.write_all(&[REPLAY_REQUEST_VERSION])?;
    writer.write_all(&[0u8; 7])?;
    writer.write_all(&request.from_seq.to_le_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        ReplayRequest, WireRecord, read_record, read_replay_request, write_record, write_replay_request,
    };
    use logjet::RecordType;

    #[test]
    fn round_trip_record() {
        let record = WireRecord {
            record_type: RecordType::Logs,
            seq: 42,
            ts_unix_ns: 77,
            payload: b"abc".to_vec(),
        };
        let mut bytes = Vec::new();
        write_record(&mut bytes, &record).unwrap();
        let decoded = read_record(&mut bytes.as_slice()).unwrap().unwrap();
        assert_eq!(decoded, record);
    }

    #[test]
    fn replay_request_round_trip() {
        let request = ReplayRequest { from_seq: 1234 };
        let mut bytes = Vec::new();
        write_replay_request(&mut bytes, &request).unwrap();
        let decoded = read_replay_request(&mut bytes.as_slice()).unwrap();
        assert_eq!(decoded, request);
    }
}
