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

use std::fs;
use std::io::Read;
use std::path::Path;

use serde_json::Value;

use crate::config::GoogleConfig;

/// Generate a random 64-char PKCE verifier from the unreserved set.
fn random_verifier() -> String {
    // 48 random bytes -> base64url (64 chars), all unreserved per RFC 7636.
    let mut buf = [0u8; 48];
    getrandom_fill(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

/// Fill `buf` with OS randomness. Uses /dev/urandom (sufficient for PKCE).
fn getrandom_fill(buf: &mut [u8]) {
    let mut f = fs::File::open("/dev/urandom").expect("open /dev/urandom");
    f.read_exact(buf).expect("read /dev/urandom");
}

/// Return a valid access token: from cache if fresh, refreshed if stale, or via
/// the full browser consent flow if there is no usable cached token.
pub fn authorize(google: &GoogleConfig, token_path: &Path) -> Result<String, String> {
    if let Some(token) = load_token(token_path) {
        if !token.is_expired(now_unix()) {
            return Ok(token.access_token);
        }
        if let Ok(refreshed) = refresh(google, &token.refresh_token) {
            save_token(token_path, &refreshed)?;
            return Ok(refreshed.access_token);
        }
        // refresh failed (revoked) — fall through to full consent.
    }
    let token = consent_flow(google)?;
    save_token(token_path, &token)?;
    Ok(token.access_token)
}

fn load_token(path: &Path) -> Option<CachedToken> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_token(path: &Path, token: &CachedToken) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(token).map_err(|e| e.to_string())?;
    fs::write(path, json).map_err(|e| e.to_string())?;
    // Best-effort 0600.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Exchange a refresh token for a fresh access token.
fn refresh(google: &GoogleConfig, refresh_token: &str) -> Result<CachedToken, String> {
    let resp = ureq::post(TOKEN_ENDPOINT)
        .send_form(&[
            ("client_id", google.client_id.as_str()),
            ("client_secret", google.client_secret.as_str()),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .map_err(|e| e.to_string())?;
    let v: Value = resp.into_json().map_err(|e| e.to_string())?;
    let access_token = v["access_token"]
        .as_str()
        .ok_or("no access_token")?
        .to_string();
    let expires_in = v["expires_in"].as_i64().unwrap_or(3600);
    Ok(CachedToken {
        access_token,
        refresh_token: refresh_token.to_string(),
        expires_at: now_unix() + expires_in,
    })
}

/// Full browser consent: start a localhost listener, open the consent URL,
/// capture the redirect code, exchange it for tokens.
fn consent_flow(google: &GoogleConfig) -> Result<CachedToken, String> {
    let server = tiny_http::Server::http("127.0.0.1:0").map_err(|e| e.to_string())?;
    let port = server.server_addr().to_ip().ok_or("no ip")?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}");

    let verifier = random_verifier();
    let challenge = pkce_challenge(&verifier);

    let auth_url = format!(
        "{AUTH_ENDPOINT}?response_type=code&client_id={cid}&redirect_uri={ru}\
         &scope={scope}&code_challenge={ch}&code_challenge_method=S256\
         &access_type=offline&prompt=consent",
        cid = urlencode(&google.client_id),
        ru = urlencode(&redirect_uri),
        scope = urlencode(SCOPE),
        ch = challenge,
    );

    println!("Opening browser for Google Photos authorization...");
    if open::that(&auth_url).is_err() {
        println!("Could not open a browser. Visit this URL:\n{auth_url}");
    }

    // Block for the redirect request and pull `code` from its query string.
    let request = server.recv().map_err(|e| e.to_string())?;
    let url = request.url().to_string();
    let code = query_param(&url, "code")
        .ok_or_else(|| format!("no authorization code in redirect: {url}"))?;
    let _ = request.respond(tiny_http::Response::from_string(
        "Authorization complete. You can close this tab.",
    ));

    exchange_code(google, &code, &redirect_uri, &verifier)
}

fn exchange_code(
    google: &GoogleConfig,
    code: &str,
    redirect_uri: &str,
    verifier: &str,
) -> Result<CachedToken, String> {
    let resp = ureq::post(TOKEN_ENDPOINT)
        .send_form(&[
            ("client_id", google.client_id.as_str()),
            ("client_secret", google.client_secret.as_str()),
            ("code", code),
            ("code_verifier", verifier),
            ("grant_type", "authorization_code"),
            ("redirect_uri", redirect_uri),
        ])
        .map_err(|e| e.to_string())?;
    let v: Value = resp.into_json().map_err(|e| e.to_string())?;
    let access_token = v["access_token"]
        .as_str()
        .ok_or("no access_token")?
        .to_string();
    let refresh_token = v["refresh_token"]
        .as_str()
        .ok_or("no refresh_token")?
        .to_string();
    let expires_in = v["expires_in"].as_i64().unwrap_or(3600);
    Ok(CachedToken {
        access_token,
        refresh_token,
        expires_at: now_unix() + expires_in,
    })
}

/// Minimal percent-encoding for query values (encode everything not unreserved).
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Extract a query parameter value from a request URL like `/?code=abc&x=y`.
fn query_param(url: &str, key: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                return Some(v.to_string());
            }
        }
    }
    None
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

    #[test]
    fn extracts_code_from_redirect_url() {
        assert_eq!(
            query_param("/?code=abc123&scope=x", "code"),
            Some("abc123".to_string())
        );
        assert_eq!(query_param("/", "code"), None);
    }
}
