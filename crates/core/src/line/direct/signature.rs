//! LINE webhook signature verification.
//!
//! LINE Platform signs every webhook delivery with
//! `X-Line-Signature: base64(HMAC-SHA256(channel_secret, raw_body))`.
//! Constant-time compare so a forged body cannot be probed via timing.

use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Verify `header_value` matches HMAC-SHA256(secret, body).
///
/// Returns `true` only when the header is a valid base64 string AND
/// the MAC matches in constant time. Any other failure (bad base64,
/// wrong length, missing, empty secret) returns `false` without leaking which.
pub fn verify(body: &[u8], header_value: &str, secret: &[u8]) -> bool {
    if secret.is_empty() {
        // A deploy with unset secret would otherwise accept anything
        // an attacker signs with the empty key. Refuse explicitly.
        return false;
    }
    let expected = match base64::engine::general_purpose::STANDARD.decode(header_value) {
        Ok(bytes) => bytes,
        Err(_) => return false,
    };
    let mut mac = match HmacSha256::new_from_slice(secret) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(body);
    mac.verify_slice(&expected).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn sign(body: &[u8], secret: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret).unwrap();
        mac.update(body);
        base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
    }

    #[test]
    fn verify_accepts_valid_signature() {
        let secret = b"channel-secret-abc";
        let body = br#"{"events":[]}"#;
        let sig = sign(body, secret);
        assert!(verify(body, &sig, secret));
    }

    #[test]
    fn verify_rejects_wrong_secret() {
        let body = br#"{"events":[]}"#;
        let sig = sign(body, b"real");
        assert!(!verify(body, &sig, b"forged"));
    }

    #[test]
    fn verify_rejects_tampered_body() {
        let secret = b"s";
        let sig = sign(br#"{"a":1}"#, secret);
        assert!(!verify(br#"{"a":2}"#, &sig, secret));
    }

    #[test]
    fn verify_rejects_non_base64_header() {
        assert!(!verify(b"body", "not::valid::base64::!!!", b"s"));
    }

    #[test]
    fn verify_rejects_empty_secret() {
        assert!(!verify(b"x", "anything", b""));
    }

    #[test]
    fn verify_rejects_empty_header() {
        assert!(!verify(b"x", "", b"s"));
    }
}
