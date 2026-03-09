use std::fs::File;
use std::io::{self, BufReader};
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;

use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::server::WebPkiClientVerifier;
use rustls::{ClientConfig, RootCertStore, ServerConfig};

use crate::config::{CollectorConfig, IngestTlsConfig, TlsConfig};

pub fn load_server_config(tls: &TlsConfig) -> io::Result<Arc<ServerConfig>> {
    load_server_config_from_parts(
        tls.cert_file.as_deref(),
        tls.key_file.as_deref(),
        tls.ca_file.as_deref(),
        tls.require_client_cert,
        "tls",
    )
}

pub fn load_ingest_server_config(tls: &IngestTlsConfig) -> io::Result<Arc<ServerConfig>> {
    load_server_config_from_parts(
        tls.cert_file.as_deref(),
        tls.key_file.as_deref(),
        tls.ca_file.as_deref(),
        tls.require_client_cert,
        "ingest",
    )
}

pub fn load_client_config(tls: &TlsConfig) -> io::Result<Arc<ClientConfig>> {
    load_client_config_from_parts(
        tls.ca_file.as_deref(),
        tls.cert_file.as_deref(),
        tls.key_file.as_deref(),
        "tls",
    )
}

pub fn load_collector_client_config(collector: &CollectorConfig) -> io::Result<Arc<ClientConfig>> {
    load_client_config_from_parts(
        collector.ca_file.as_deref(),
        collector.cert_file.as_deref(),
        collector.key_file.as_deref(),
        "collector",
    )
}

pub fn parse_server_name(tls: &TlsConfig, authority: &str) -> io::Result<ServerName<'static>> {
    parse_server_name_override(tls.server_name.as_deref(), authority)
}

pub fn parse_collector_server_name(
    collector: &CollectorConfig,
    authority: &str,
) -> io::Result<ServerName<'static>> {
    parse_server_name_override(collector.server_name.as_deref(), authority)
}

pub fn authority_host(authority: &str) -> &str {
    if let Some(rest) = authority.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(authority);
    }
    authority
        .rsplit_once(':')
        .map(|(host, _)| host)
        .unwrap_or(authority)
}

fn parse_server_name_override(
    override_name: Option<&str>,
    authority: &str,
) -> io::Result<ServerName<'static>> {
    let name = override_name.unwrap_or_else(|| authority_host(authority));
    if let Ok(ip) = name.parse::<IpAddr>() {
        return Ok(ServerName::IpAddress(ip.into()));
    }

    ServerName::try_from(name.to_string())
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))
}

fn load_server_config_from_parts(
    cert_file: Option<&Path>,
    key_file: Option<&Path>,
    ca_file: Option<&Path>,
    require_client_cert: bool,
    namespace: &str,
) -> io::Result<Arc<ServerConfig>> {
    let cert_file = cert_file.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{namespace}.cert-file is required when {namespace}.tls-enable or {namespace}.enable is true"),
        )
    })?;
    let key_file = key_file.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{namespace}.key-file is required when {namespace}.tls-enable or {namespace}.enable is true"),
        )
    })?;

    let certs = load_certs(cert_file)?;
    let key = load_private_key(key_file)?;
    let builder = ServerConfig::builder();
    let server_config = if require_client_cert {
        let ca_file = ca_file.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{namespace}.ca-file is required when client certificates are required"),
            )
        })?;
        let roots = load_root_store(ca_file)?;
        let verifier = WebPkiClientVerifier::builder(Arc::new(roots))
            .build()
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))?;
        builder
            .with_client_cert_verifier(verifier)
            .with_single_cert(certs, key)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))?
    } else {
        builder
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))?
    };

    Ok(Arc::new(server_config))
}

fn load_client_config_from_parts(
    ca_file: Option<&Path>,
    cert_file: Option<&Path>,
    key_file: Option<&Path>,
    namespace: &str,
) -> io::Result<Arc<ClientConfig>> {
    let ca_file = ca_file.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{namespace}.ca-file is required for TLS client mode"),
        )
    })?;
    let roots = load_root_store(ca_file)?;
    let builder = ClientConfig::builder().with_root_certificates(roots);

    let client_config = match (cert_file, key_file) {
        (Some(cert_file), Some(key_file)) => builder
            .with_client_auth_cert(load_certs(cert_file)?, load_private_key(key_file)?)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))?,
        (None, None) => builder.with_no_client_auth(),
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "{namespace}.cert-file and {namespace}.key-file must either both be set or both be unset"
                ),
            ));
        }
    };

    Ok(Arc::new(client_config))
}

fn load_root_store(path: &Path) -> io::Result<RootCertStore> {
    let certs = load_certs(path)?;
    let mut roots = RootCertStore::empty();
    let (_added, ignored) = roots.add_parsable_certificates(certs);
    if ignored > 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "failed to parse {ignored} CA certificate(s) from {}",
                path.display()
            ),
        ));
    }
    Ok(roots)
}

fn load_certs(path: &Path) -> io::Result<Vec<CertificateDer<'static>>> {
    let mut reader = BufReader::new(File::open(path)?);
    rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))
}

fn load_private_key(path: &Path) -> io::Result<PrivateKeyDer<'static>> {
    let mut reader = BufReader::new(File::open(path)?);
    rustls_pemfile::private_key(&mut reader)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))?
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("no private key found in {}", path.display()),
            )
        })
}

#[cfg(test)]
#[path = "tls_utst.rs"]
mod tls_utst;
