use super::{ConnectionLimiter, read_http_request, write_http_response};
use std::io::Cursor;
use std::sync::Arc;

#[test]
fn read_http_request_parses_valid_request() {
    let bytes = b"POST /v1/logs HTTP/1.1\r\nHost: example\r\nContent-Length: 3\r\n\r\nabc";
    let mut cursor = Cursor::new(bytes.as_slice());
    let request = read_http_request(&mut cursor, 1024).unwrap();
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/v1/logs");
    assert_eq!(request.body, b"abc");
}

#[test]
fn read_http_request_rejects_missing_content_length() {
    let bytes = b"POST /v1/logs HTTP/1.1\r\nHost: example\r\n\r\nabc";
    let mut cursor = Cursor::new(bytes.as_slice());
    let err = read_http_request(&mut cursor, 1024).err().unwrap();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn read_http_request_rejects_short_body() {
    let bytes = b"POST /v1/logs HTTP/1.1\r\nHost: example\r\nContent-Length: 5\r\n\r\nabc";
    let mut cursor = Cursor::new(bytes.as_slice());
    let err = read_http_request(&mut cursor, 1024).err().unwrap();
    assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
}

#[test]
fn read_http_request_rejects_invalid_request_line() {
    let bytes = b"POST\r\nHost: example\r\nContent-Length: 0\r\n\r\n";
    let mut cursor = Cursor::new(bytes.as_slice());
    let err = read_http_request(&mut cursor, 1024).err().unwrap();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn read_http_request_rejects_payloads_over_limit() {
    let bytes = b"POST /v1/logs HTTP/1.1\r\nHost: example\r\nContent-Length: 6\r\n\r\nabcdef";
    let mut cursor = Cursor::new(bytes.as_slice());
    let err = read_http_request(&mut cursor, 5).err().unwrap();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(err.to_string(), "payload too large");
}

#[test]
fn write_http_response_writes_status_line() {
    let mut bytes = Vec::new();
    write_http_response(&mut bytes, 404, "not found").unwrap();
    let response = String::from_utf8(bytes).unwrap();
    assert!(response.starts_with("HTTP/1.1 404 Not Found\r\n"));
    assert!(response.ends_with("not found"));
}

#[test]
fn write_http_response_supports_payload_too_large_status() {
    let mut bytes = Vec::new();
    write_http_response(&mut bytes, 413, "payload too large").unwrap();
    let response = String::from_utf8(bytes).unwrap();
    assert!(response.starts_with("HTTP/1.1 413 Payload Too Large\r\n"));
    assert!(response.ends_with("payload too large"));
}

#[test]
fn write_http_response_supports_service_unavailable_status() {
    let mut bytes = Vec::new();
    write_http_response(&mut bytes, 503, "busy").unwrap();
    let response = String::from_utf8(bytes).unwrap();
    assert!(response.starts_with("HTTP/1.1 503 Service Unavailable\r\n"));
    assert!(response.ends_with("busy"));
}

#[test]
fn connection_limiter_rejects_when_max_clients_is_reached() {
    let limiter = Arc::new(ConnectionLimiter::new(1));
    let first = limiter.try_acquire();
    let second = limiter.try_acquire();

    assert!(first.is_some());
    assert!(second.is_none());
}

#[test]
fn connection_limiter_accepts_again_after_permit_is_dropped() {
    let limiter = Arc::new(ConnectionLimiter::new(1));
    let first = limiter.try_acquire();
    assert!(first.is_some());
    drop(first);

    let second = limiter.try_acquire();
    assert!(second.is_some());
}
