//! OAuth 2.0 desktop flow for the Google Photos Library API: PKCE consent in
//! the browser, exchange for tokens, and a refresh-token cache so repeat runs
//! are silent.

use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Refresh tokens are requested ~60s before the access token actually expires,
/// to avoid using a token that dies mid-request.
const EXPIRY_LEEWAY_SECS: i64 = 60;

pub const SCOPE: &str = "https://www.googleapis.com/auth/photoslibrary.appendonly";
const AUTH_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";

#[derive(Debug, Serialize, Deserialize)]
pub struct CachedToken {
    pub access_token: String,
    pub refresh_token: String,
    /// Unix seconds at which `access_token` stops being valid.
    pub expires_at: i64,
}

impl CachedToken {
    pub fn is_expired(&self, now: i64) -> bool {
        now >= self.expires_at - EXPIRY_LEEWAY_SECS
    }
}

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// PKCE S256 challenge: base64url(sha256(verifier)), no padding.
pub fn pkce_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_is_base64url_sha256_of_verifier() {
        // RFC 7636 Appendix B test vector.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = pkce_challenge(verifier);
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn token_is_expired_within_leeway() {
        // expires_at 30s from now, 60s leeway => treated as expired.
        let token = CachedToken {
            access_token: "a".into(),
            refresh_token: "r".into(),
            expires_at: now_unix() + 30,
        };
        assert!(token.is_expired(now_unix()));
    }

    #[test]
    fn fresh_token_is_not_expired() {
        let token = CachedToken {
            access_token: "a".into(),
            refresh_token: "r".into(),
            expires_at: now_unix() + 3600,
        };
        assert!(!token.is_expired(now_unix()));
    }
}
