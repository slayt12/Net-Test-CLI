//! Server-side identity: load `cert.pem` + `key.pem` from a directory, or generate a self-signed
//! pair on first run. The fingerprint is surfaced so operators can hand it to clients.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustls::ServerConfig;
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};

use super::fingerprint::{self, Fingerprint};

pub struct ServerIdentity {
    pub config: Arc<ServerConfig>,
    pub fingerprint: Fingerprint,
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
    pub generated: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("io {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("certificate generation: {0}")]
    Gen(#[from] rcgen::Error),
    #[error("pem parse {path}: {source}")]
    Pem {
        path: PathBuf,
        #[source]
        source: rustls_pki_types::pem::Error,
    },
    #[error("tls config: {0}")]
    Tls(#[from] rustls::Error),
}

pub fn load_or_generate(dir: &Path, hostnames: &[String]) -> Result<ServerIdentity, IdentityError> {
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    let io = |path: &Path| {
        let p = path.to_path_buf();
        move |source| IdentityError::Io { path: p, source }
    };

    let generated = !(cert_path.exists() && key_path.exists());
    if generated {
        std::fs::create_dir_all(dir).map_err(io(dir))?;
        let mut names: Vec<String> = hostnames.to_vec();
        if !names.iter().any(|n| n == "localhost") {
            names.push("localhost".into());
        }
        let ck = rcgen::generate_simple_self_signed(names)?;
        std::fs::write(&cert_path, ck.cert.pem()).map_err(io(&cert_path))?;
        std::fs::write(&key_path, ck.signing_key.serialize_pem()).map_err(io(&key_path))?;
        restrict_permissions(&key_path);
    }

    let cert_pem = std::fs::read(&cert_path).map_err(io(&cert_path))?;
    let key_pem = std::fs::read(&key_path).map_err(io(&key_path))?;
    let cert = CertificateDer::from_pem_slice(&cert_pem).map_err(|source| IdentityError::Pem {
        path: cert_path.clone(),
        source,
    })?;
    let key = PrivateKeyDer::from_pem_slice(&key_pem).map_err(|source| IdentityError::Pem {
        path: key_path.clone(),
        source,
    })?;
    let fp = fingerprint::of_der(cert.as_ref());

    let config = ServerConfig::builder_with_provider(super::provider())
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)?;

    Ok(ServerIdentity {
        config: Arc::new(config),
        fingerprint: fp,
        cert_path,
        key_path,
        generated,
    })
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}
