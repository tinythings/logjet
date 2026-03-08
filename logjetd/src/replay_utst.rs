use super::{CollectorEndpoint, read_bridge_state, write_bridge_state};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn host_port_defaults_to_v1_logs() {
    let endpoint = CollectorEndpoint::parse("127.0.0.1:4318").unwrap();
    assert_eq!(endpoint.authority, "127.0.0.1:4318");
    assert_eq!(endpoint.path, "/v1/logs");
}

#[test]
fn http_url_with_custom_path_is_preserved() {
    let endpoint = CollectorEndpoint::parse("http://127.0.0.1:4318/custom/path").unwrap();
    assert_eq!(endpoint.authority, "127.0.0.1:4318");
    assert_eq!(endpoint.path, "/custom/path");
}

#[test]
fn http_url_without_leading_slash_is_normalised() {
    let endpoint = CollectorEndpoint::parse("http://127.0.0.1:4318/custom").unwrap();
    assert_eq!(endpoint.path, "/custom");
}

#[test]
fn https_url_is_supported() {
    let endpoint = CollectorEndpoint::parse("https://127.0.0.1:4318/v1/logs").unwrap();
    assert_eq!(endpoint.authority, "127.0.0.1:4318");
    assert_eq!(endpoint.path, "/v1/logs");
    assert!(endpoint.tls);
}

#[test]
fn missing_authority_is_rejected() {
    let err = CollectorEndpoint::parse("http:///v1/logs")
        .err()
        .unwrap()
        .to_string();
    assert!(err.contains("missing host:port"));
}

#[test]
fn bridge_state_round_trip() {
    let path = unique_temp_path("bridge-state");
    assert_eq!(read_bridge_state(Some(&path)).unwrap(), 0);
    write_bridge_state(Some(&path), 42).unwrap();
    assert_eq!(read_bridge_state(Some(&path)).unwrap(), 42);
    fs::remove_file(path).unwrap();
}

#[test]
fn invalid_bridge_state_is_rejected() {
    let path = unique_temp_path("bridge-state-invalid");
    fs::write(&path, "not-a-number").unwrap();
    let err = read_bridge_state(Some(&path)).err().unwrap();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    fs::remove_file(path).unwrap();
}

fn unique_temp_path(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("logjetd-{label}-{nanos}-{}.state", std::process::id()))
}
