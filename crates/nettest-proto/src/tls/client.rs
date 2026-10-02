//! Client-side rustls config: either pinned to a server fingerprint or (explicitly) insecure.
//!
//! Both verifiers still check the handshake signatures, so the only thing skipped is chain
//! validation against a CA store, which a self-signed cert could never pass anyway.

use std::sync::Arc;

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
}

pub fn client_config(mode: TlsClientMode) -> Result<Arc<ClientConfig>, rustls::Error> {
    let provider = super::provider();
    let verifier: Arc<dyn ServerCertVerifier> = match mode {
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
