use super::{authority_host, load_client_config, load_ingest_server_config, load_server_config, parse_server_name};
use crate::config::{IngestTlsConfig, TlsConfig};
use std::path::PathBuf;
use std::sync::OnceLock;

fn ensure_rustls_provider() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        rustls::crypto::ring::default_provider().install_default().expect("install rustls ring provider");
    });
}

fn demo_cert_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("demo").join("remote-drain-tls").join("certs").join(name)
}

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
    let tls = TlsConfig { enable: true, ca_file: None, cert_file: None, key_file: None, require_client_cert: false, server_name: None };

    parse_server_name(&tls, "127.0.0.1:7002").unwrap();
}

#[test]
fn load_client_config_requires_ca_file() {
    let tls = TlsConfig { enable: true, ca_file: None, cert_file: None, key_file: None, require_client_cert: false, server_name: None };

    let err = load_client_config(&tls).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
}

#[test]
fn load_server_config_requires_cert_and_key() {
    let tls = TlsConfig { enable: true, ca_file: None, cert_file: None, key_file: None, require_client_cert: false, server_name: None };

    let err = load_server_config(&tls).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
}

#[test]
fn load_server_config_requires_ca_when_client_certs_required() {
    ensure_rustls_provider();
    let tls = TlsConfig {
        enable: true,
        ca_file: None,
        cert_file: Some(demo_cert_path("appliance.pem")),
        key_file: Some(demo_cert_path("appliance.key")),
        require_client_cert: true,
        server_name: None,
    };

    let err = load_server_config(&tls).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
}

#[test]
fn load_ingest_server_config_requires_cert_and_key() {
    let tls = IngestTlsConfig { enable: true, ca_file: None, cert_file: None, key_file: None, require_client_cert: false };

    let err = load_ingest_server_config(&tls).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
}
