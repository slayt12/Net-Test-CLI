//! Client-side rustls config: pinned to a server fingerprint, verified against the Mozilla root
//! bundle (webpki-roots), or (explicitly) insecure.
//!
//! The pinned and insecure verifiers still check the handshake signatures, so the only thing
//! skipped is chain validation against a CA store, which a self-signed cert could never pass
//! anyway. `WebPki` is for public endpoints (the monitor's ntfy / Slack / Discord webhooks); it
//! deliberately does not consult the operating system's store so behaviour is identical on
//! every host.

use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::{CertificateError, ClientConfig, DigitallySignedStruct, SignatureScheme};
use rustls_pki_types::{CertificateDer, ServerName, UnixTime};

use super::fingerprint::{self, Fingerprint};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsClientMode {
    /// Accept any certificate. Loudly opt-in only.
    Insecure,
    /// Accept only the certificate whose SHA-256 matches.
    Pinned(Fingerprint),
    /// Full chain validation against the bundled Mozilla roots (public CAs only).
    WebPki,
}

pub fn client_config(mode: TlsClientMode) -> Result<Arc<ClientConfig>, rustls::Error> {
    let provider = super::provider();
    let verifier: Arc<dyn ServerCertVerifier> = match mode {
        TlsClientMode::WebPki => return Ok(webpki_config()),
        TlsClientMode::Insecure => Arc::new(Verifier {
            pin: None,
            provider: provider.clone(),
        }),
        TlsClientMode::Pinned(fp) => Arc::new(Verifier {
            pin: Some(fp),
            provider: provider.clone(),
        }),
    };
    let cfg = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    Ok(Arc::new(cfg))
}

/// Connect to `host:port`, complete a TLS handshake accepting any certificate, and return the
/// SHA-256 of the leaf certificate the server presented plus the address it was fetched from.
/// Used by the TUI to fill a `fingerprint` field; the caller must tell the user to compare the
/// value with the one the server prints, since a fetch cannot tell a legitimate server from an
/// interceptor (trust on first use).
pub async fn peer_fingerprint(
    host: &str,
    port: u16,
    prefer_ipv6: Option<bool>,
    timeout: Duration,
) -> Result<(Fingerprint, SocketAddr), String> {
    let (addr, _) = crate::transport::resolve(host, port, prefer_ipv6, timeout)
        .await
        .map_err(|e| e.to_string())?;
    let tcp = tokio::time::timeout(timeout, tokio::net::TcpStream::connect(addr))
        .await
        .map_err(|_| format!("tcp connect to {addr}: timed out after {timeout:?}"))?
        .map_err(|e| format!("tcp connect to {addr}: {e}"))?;
    let cfg = client_config(TlsClientMode::Insecure).map_err(|e| e.to_string())?;
    let name = ServerName::try_from(host.trim_matches(['[', ']']).to_string())
        .map_err(|_| format!("'{host}' is not a valid TLS server name"))?;
    let tls = tokio::time::timeout(
        timeout,
        tokio_rustls::TlsConnector::from(cfg).connect(name, tcp),
    )
    .await
    .map_err(|_| format!("tls handshake with {addr}: timed out after {timeout:?}"))?
    .map_err(|e| format!("tls handshake with {addr}: {e}"))?;
    let der = tls
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|c| c.first())
        .ok_or_else(|| format!("{addr} sent no certificate"))?;
    Ok((fingerprint::of_der(der.as_ref()), addr))
}

/// Built once: the root store holds ~150 anchors and every webhook would otherwise rebuild it.
fn webpki_config() -> Arc<ClientConfig> {
    static CFG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    CFG.get_or_init(|| {
        let roots = rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        let cfg = ClientConfig::builder_with_provider(super::provider())
            .with_safe_default_protocol_versions()
            .expect("ring provider supports the default protocol versions")
            .with_root_certificates(roots)
            .with_no_client_auth();
        Arc::new(cfg)
    })
    .clone()
}

#[derive(Debug)]
struct Verifier {
    pin: Option<Fingerprint>,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl ServerCertVerifier for Verifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match self.pin {
            None => Ok(ServerCertVerified::assertion()),
            Some(expected) => {
                let got = fingerprint::of_der(end_entity.as_ref());
                if got == expected {
                    Ok(ServerCertVerified::assertion())
                } else {
                    Err(rustls::Error::InvalidCertificate(
                        CertificateError::ApplicationVerificationFailure,
                    ))
                }
            }
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webpki_config_builds_and_is_shared() {
        let a = client_config(TlsClientMode::WebPki).unwrap();
        let b = client_config(TlsClientMode::WebPki).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert!(client_config(TlsClientMode::Insecure).is_ok());
    }
}
