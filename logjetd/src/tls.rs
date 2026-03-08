use std::fs::File;
use std::io::{self, BufReader};
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;

use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::server::WebPkiClientVerifier;
use rustls::{ClientConfig, RootCertStore, ServerConfig};

use crate::config::TlsConfig;

pub fn load_server_config(tls: &TlsConfig) -> io::Result<Arc<ServerConfig>> {
    let cert_file = tls
        .cert_file
        .as_deref()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "tls.cert-file is required when tls.enable is true"))?;
    let key_file = tls
        .key_file
        .as_deref()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "tls.key-file is required when tls.enable is true"))?;

    let certs = load_certs(cert_file)?;
    let key = load_private_key(key_file)?;

    let builder = ServerConfig::builder();
    let server_config = if tls.require_client_cert {
        let ca_file = tls.ca_file.as_deref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "tls.ca-file is required when tls.require-client-cert is true",
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

pub fn load_client_config(tls: &TlsConfig) -> io::Result<Arc<ClientConfig>> {
    let ca_file = tls
        .ca_file
        .as_deref()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "tls.ca-file is required when tls.enable is true"))?;
    let roots = load_root_store(ca_file)?;
    let builder = ClientConfig::builder().with_root_certificates(roots);

    let client_config = match (tls.cert_file.as_deref(), tls.key_file.as_deref()) {
        (Some(cert_file), Some(key_file)) => builder
            .with_client_auth_cert(load_certs(cert_file)?, load_private_key(key_file)?)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))?,
        (None, None) => builder.with_no_client_auth(),
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "tls.cert-file and tls.key-file must either both be set or both be unset",
            ));
        }
    };

    Ok(Arc::new(client_config))
}

pub fn parse_server_name(tls: &TlsConfig, authority: &str) -> io::Result<ServerName<'static>> {
    let name = tls
        .server_name
        .as_deref()
        .unwrap_or_else(|| authority_host(authority));

    if let Ok(ip) = name.parse::<IpAddr>() {
        return Ok(ServerName::IpAddress(ip.into()));
    }

    ServerName::try_from(name.to_string())
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))
}

fn authority_host(authority: &str) -> &str {
    if let Some(rest) = authority.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(authority);
    }
    authority.rsplit_once(':').map(|(host, _)| host).unwrap_or(authority)
}

fn load_root_store(path: &Path) -> io::Result<RootCertStore> {
    let certs = load_certs(path)?;
    let mut roots = RootCertStore::empty();
    let (_added, ignored) = roots.add_parsable_certificates(certs);
    if ignored > 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("failed to parse {ignored} CA certificate(s) from {}", path.display()),
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
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("no private key found in {}", path.display())))
}

#[cfg(test)]
#[path = "tls_utst.rs"]
mod tls_utst;
