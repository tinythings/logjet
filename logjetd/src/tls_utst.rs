use super::{authority_host, parse_server_name};
use crate::config::TlsConfig;

#[test]
fn authority_host_strips_port() {
    assert_eq!(authority_host("example.com:7002"), "example.com");
}

#[test]
fn authority_host_handles_bracketed_ipv6() {
    assert_eq!(authority_host("[2001:db8::1]:7002"), "2001:db8::1");
}

#[test]
fn parse_server_name_uses_override() {
    let tls = TlsConfig {
        enable: true,
        ca_file: None,
        cert_file: None,
        key_file: None,
        require_client_cert: false,
        server_name: Some("appliance.internal".to_string()),
    };

    let server_name = parse_server_name(&tls, "127.0.0.1:7002").unwrap();
    assert_eq!(server_name.to_str(), "appliance.internal");
}

#[test]
fn parse_server_name_accepts_ip_authority() {
    let tls = TlsConfig {
        enable: true,
        ca_file: None,
        cert_file: None,
        key_file: None,
        require_client_cert: false,
        server_name: None,
    };

    parse_server_name(&tls, "127.0.0.1:7002").unwrap();
}
