//! Pre-shared token check. Constant-time comparison is overkill for a diagnostics tool but is
//! free, so it is used anyway.

pub fn token_ok(expected: &str, presented: Option<&str>) -> bool {
    if expected.is_empty() {
        return true;
    }
    let Some(p) = presented else { return false };
    let a = expected.as_bytes();
    let b = p.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::token_ok;

    #[test]
    fn token_rules() {
        assert!(token_ok("", None));
        assert!(token_ok("", Some("anything")));
        assert!(token_ok("s3cret", Some("s3cret")));
        assert!(!token_ok("s3cret", Some("S3CRET")));
        assert!(!token_ok("s3cret", None));
    }
}
