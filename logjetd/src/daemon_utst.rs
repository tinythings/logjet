use super::{read_http_request, write_http_response};
use std::io::Cursor;

#[test]
fn read_http_request_parses_valid_request() {
    let bytes = b"POST /v1/logs HTTP/1.1\r\nHost: example\r\nContent-Length: 3\r\n\r\nabc";
    let mut cursor = Cursor::new(bytes.as_slice());
    let request = read_http_request(&mut cursor).unwrap();
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/v1/logs");
    assert_eq!(request.body, b"abc");
}

#[test]
fn read_http_request_rejects_missing_content_length() {
    let bytes = b"POST /v1/logs HTTP/1.1\r\nHost: example\r\n\r\nabc";
    let mut cursor = Cursor::new(bytes.as_slice());
    let err = read_http_request(&mut cursor).err().unwrap();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn read_http_request_rejects_short_body() {
    let bytes = b"POST /v1/logs HTTP/1.1\r\nHost: example\r\nContent-Length: 5\r\n\r\nabc";
    let mut cursor = Cursor::new(bytes.as_slice());
    let err = read_http_request(&mut cursor).err().unwrap();
    assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
}

#[test]
fn read_http_request_rejects_invalid_request_line() {
    let bytes = b"POST\r\nHost: example\r\nContent-Length: 0\r\n\r\n";
    let mut cursor = Cursor::new(bytes.as_slice());
    let err = read_http_request(&mut cursor).err().unwrap();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn write_http_response_writes_status_line() {
    let mut bytes = Vec::new();
    write_http_response(&mut bytes, 404, "not found").unwrap();
    let response = String::from_utf8(bytes).unwrap();
    assert!(response.starts_with("HTTP/1.1 404 Not Found\r\n"));
    assert!(response.ends_with("not found"));
}
