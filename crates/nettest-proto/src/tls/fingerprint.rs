//! SHA-256 certificate fingerprints in the colon-separated hex form browsers display.

use sha2::{Digest, Sha256};

pub type Fingerprint = [u8; 32];

pub fn of_der(der: &[u8]) -> Fingerprint {
    Sha256::digest(der).into()
}

pub fn to_hex(fp: &Fingerprint) -> String {
    let mut s = String::with_capacity(95);
    for (i, b) in fp.iter().enumerate() {
        if i > 0 {
            s.push(':');
        }
        s.push_str(&format!("{b:02X}"));
    }
    s
}

/// Accepts `AB:CD:...`, `abcd...`, with or without separators / `sha256/` prefix.
pub fn parse(s: &str) -> Result<Fingerprint, String> {
    let s = s.trim();
    let s = s
        .strip_prefix("sha256/")
        .or_else(|| s.strip_prefix("SHA256:"))
        .unwrap_or(s);
    let hex: String = s
        .chars()
        .filter(|c| !matches!(c, ':' | ' ' | '-'))
        .collect();
    if hex.len() != 64 {
        return Err(format!(
            "fingerprint must be 64 hex digits (32 bytes), got {} digits",
            hex.len()
        ));
    }
    let mut out = [0u8; 32];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        let pair = std::str::from_utf8(chunk).map_err(|_| "non-ascii fingerprint")?;
        out[i] = u8::from_str_radix(pair, 16).map_err(|_| format!("bad hex '{pair}'"))?;
    }
    Ok(out)
}

/// Fingerprint of the first certificate in a PEM file (what the server writes to `cert.pem`).
pub fn of_pem(pem: &[u8]) -> Option<Fingerprint> {
    use rustls_pki_types::pem::PemObject;
    let cert = rustls_pki_types::CertificateDer::from_pem_slice(pem).ok()?;
    Some(of_der(cert.as_ref()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        let fp = of_der(b"hello");
        let hex = to_hex(&fp);
        assert_eq!(hex.len(), 95);
        assert_eq!(parse(&hex).unwrap(), fp);
        assert_eq!(parse(&hex.replace(':', "").to_lowercase()).unwrap(), fp);
        assert!(parse("abcd").is_err());
    }

    #[test]
    fn known_vector() {
        // sha256("") = e3b0c442...
        assert!(to_hex(&of_der(b"")).starts_with("E3:B0:C4:42"));
    }
}
