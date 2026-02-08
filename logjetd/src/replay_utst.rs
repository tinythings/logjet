use super::CollectorEndpoint;

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
fn http_url_without_leading_slash_is_normalized() {
    let endpoint = CollectorEndpoint::parse("http://127.0.0.1:4318/custom").unwrap();
    assert_eq!(endpoint.path, "/custom");
}

#[test]
fn https_is_rejected() {
    let err = CollectorEndpoint::parse("https://127.0.0.1:4318/v1/logs")
        .err()
        .unwrap()
        .to_string();
    assert!(err.contains("https"));
}

#[test]
fn missing_authority_is_rejected() {
    let err = CollectorEndpoint::parse("http:///v1/logs")
        .err()
        .unwrap()
        .to_string();
    assert!(err.contains("missing host:port"));
}
