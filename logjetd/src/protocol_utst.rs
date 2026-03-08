use super::{
    ReplayAck, ReplayRequest, WireRecord, read_record, read_record_with_limit, read_replay_ack, read_replay_request,
    write_record, write_replay_ack, write_replay_request,
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
    let request = ReplayRequest {
        from_seq: 1234,
        consume: true,
    };
    let mut bytes = Vec::new();
    write_replay_request(&mut bytes, &request).unwrap();
    let decoded = read_replay_request(&mut bytes.as_slice()).unwrap();
    assert_eq!(decoded, request);
}

#[test]
fn replay_ack_round_trip() {
    let ack = ReplayAck { ack_seq: 9876 };
    let mut bytes = Vec::new();
    write_replay_ack(&mut bytes, &ack).unwrap();
    let decoded = read_replay_ack(&mut bytes.as_slice()).unwrap();
    assert_eq!(decoded, ack);
}

#[test]
fn invalid_record_magic_is_rejected() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"BADMAGIC");
    bytes.extend_from_slice(&[1u8; 24]);
    let err = read_record(&mut bytes.as_slice()).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn unsupported_record_version_is_rejected() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"LJNETV01");
    bytes.push(9);
    bytes.push(RecordType::Logs as u8);
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes.extend_from_slice(&42u64.to_le_bytes());
    bytes.extend_from_slice(&77u64.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());

    let err = read_record(&mut bytes.as_slice()).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn record_payload_over_limit_is_rejected() {
    let record = WireRecord {
        record_type: RecordType::Logs,
        seq: 42,
        ts_unix_ns: 77,
        payload: b"abcdef".to_vec(),
    };
    let mut bytes = Vec::new();
    write_record(&mut bytes, &record).unwrap();
    let err = read_record_with_limit(&mut bytes.as_slice(), 5).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn invalid_replay_request_magic_is_rejected() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"BADRPL01");
    bytes.extend_from_slice(&[1]);
    bytes.extend_from_slice(&[0u8; 7]);
    bytes.extend_from_slice(&99u64.to_le_bytes());

    let err = read_replay_request(&mut bytes.as_slice()).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn invalid_replay_ack_magic_is_rejected() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"BADACK01");
    bytes.extend_from_slice(&[1]);
    bytes.extend_from_slice(&[0u8; 7]);
    bytes.extend_from_slice(&99u64.to_le_bytes());

    let err = read_replay_ack(&mut bytes.as_slice()).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn unsupported_replay_ack_version_is_rejected() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"LJRPA001");
    bytes.extend_from_slice(&[9]);
    bytes.extend_from_slice(&[0u8; 7]);
    bytes.extend_from_slice(&99u64.to_le_bytes());

    let err = read_replay_ack(&mut bytes.as_slice()).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn unsupported_replay_request_version_is_rejected() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"LJRPL001");
    bytes.extend_from_slice(&[9]);
    bytes.extend_from_slice(&[0u8; 7]);
    bytes.extend_from_slice(&99u64.to_le_bytes());

    let err = read_replay_request(&mut bytes.as_slice()).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}
