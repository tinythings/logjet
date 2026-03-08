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
