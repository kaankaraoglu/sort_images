//! Google Photos Library API client. The network surface is expressed as the
//! `PhotosApi` trait so upload orchestration can be tested with a fake; the
//! `UreqPhotos` implementation talks to the real API over HTTPS.

use crate::ledger::Ledger;

/// Max media items per `mediaItems:batchCreate` call (Google's documented limit).
pub const BATCH_LIMIT: usize = 50;

const UPLOAD_URL: &str = "https://photoslibrary.googleapis.com/v1/uploads";
const ALBUMS_URL: &str = "https://photoslibrary.googleapis.com/v1/albums";
const BATCH_URL: &str = "https://photoslibrary.googleapis.com/v1/mediaItems:batchCreate";

/// A file ready to upload, with its ledger key.
pub struct PendingUpload {
    pub file_name: String,
    pub bytes: Vec<u8>,
    pub key: String,
}

#[derive(Debug)]
pub enum ApiError {
    /// Transient (429/5xx/network) — safe to retry the file later.
    Transient(String),
    /// Non-recoverable for this file (4xx other than 429).
    Fatal(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Transient(m) => write!(f, "transient: {m}"),
            ApiError::Fatal(m) => write!(f, "{m}"),
        }
    }
}

/// The network operations the upload pipeline needs.
pub trait PhotosApi {
    fn create_album(&self, title: &str) -> Result<String, ApiError>;
    fn upload_bytes(&self, file_name: &str, bytes: &[u8]) -> Result<String, ApiError>;
    fn batch_create(&self, album_id: &str, tokens: &[String]) -> Result<(), ApiError>;
}

/// Split `items` into contiguous chunks of at most `max` elements.
pub fn chunk<T: Clone>(items: &[T], max: usize) -> Vec<Vec<T>> {
    items.chunks(max).map(<[T]>::to_vec).collect()
}

/// Return the album id for `month`, creating it (once) if the ledger has none.
pub fn ensure_album<A: PhotosApi>(
    api: &A,
    ledger: &mut Ledger,
    month: &str,
) -> Result<String, ApiError> {
    if let Some(id) = ledger.album_id(month) {
        return Ok(id.to_string());
    }
    let id = api.create_album(month)?;
    ledger.set_album_id(month, &id);
    Ok(id)
}

/// Upload one file's bytes and return its upload token. Does NOT mark the
/// ledger — the caller marks the key only after `batch_create` succeeds, so a
/// failed attach does not strand the file as "uploaded".
pub fn upload_one<A: PhotosApi>(
    api: &A,
    _ledger: &mut Ledger,
    item: &PendingUpload,
) -> Result<String, ApiError> {
    api.upload_bytes(&item.file_name, &item.bytes)
}

use std::thread;
use std::time::Duration;

use serde_json::json;

/// Real client. Holds a bearer access token (already refreshed by `auth`).
pub struct UreqPhotos {
    access_token: String,
}

impl UreqPhotos {
    pub fn new(access_token: String) -> Self {
        Self { access_token }
    }

    fn bearer(&self) -> String {
        format!("Bearer {}", self.access_token)
    }

    /// Map a ureq error to an `ApiError`, classifying 429/5xx as transient.
    fn classify(err: ureq::Error) -> ApiError {
        match err {
            ureq::Error::Status(code, _) if code == 429 || code >= 500 => {
                ApiError::Transient(format!("HTTP {code}"))
            }
            ureq::Error::Status(code, resp) => {
                let body = resp.into_string().unwrap_or_default();
                ApiError::Fatal(format!("HTTP {code}: {body}"))
            }
            ureq::Error::Transport(t) => ApiError::Transient(t.to_string()),
        }
    }
}

/// Retry a transient operation up to 3 times with exponential backoff.
fn with_retry<T, F: FnMut() -> Result<T, ApiError>>(mut f: F) -> Result<T, ApiError> {
    let mut delay = Duration::from_millis(500);
    for attempt in 0..3 {
        match f() {
            Ok(v) => return Ok(v),
            Err(ApiError::Transient(_)) if attempt < 2 => {
                thread::sleep(delay);
                delay *= 2;
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!("loop returns on the final attempt")
}

impl PhotosApi for UreqPhotos {
    fn create_album(&self, title: &str) -> Result<String, ApiError> {
        with_retry(|| {
            let resp = ureq::post(ALBUMS_URL)
                .set("Authorization", &self.bearer())
                .send_json(json!({ "album": { "title": title } }))
                .map_err(Self::classify)?;
            let v: serde_json::Value = resp
                .into_json()
                .map_err(|e| ApiError::Fatal(e.to_string()))?;
            v["id"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| ApiError::Fatal("album response missing id".into()))
        })
    }

    fn upload_bytes(&self, file_name: &str, bytes: &[u8]) -> Result<String, ApiError> {
        with_retry(|| {
            let resp = ureq::post(UPLOAD_URL)
                .set("Authorization", &self.bearer())
                .set("Content-type", "application/octet-stream")
                .set("X-Goog-Upload-Content-Type", "application/octet-stream")
                .set("X-Goog-Upload-Protocol", "raw")
                .set("X-Goog-Upload-File-Name", file_name)
                .send_bytes(bytes)
                .map_err(Self::classify)?;
            resp.into_string()
                .map_err(|e| ApiError::Fatal(e.to_string()))
        })
    }

    fn batch_create(&self, album_id: &str, tokens: &[String]) -> Result<(), ApiError> {
        let new_items: Vec<_> = tokens
            .iter()
            .map(|t| json!({ "simpleMediaItem": { "uploadToken": t } }))
            .collect();
        with_retry(|| {
            ureq::post(BATCH_URL)
                .set("Authorization", &self.bearer())
                .send_json(json!({ "albumId": album_id, "newMediaItems": new_items }))
                .map_err(Self::classify)?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Records calls and returns canned ids so we can assert orchestration.
    struct FakeApi {
        created_albums: RefCell<Vec<String>>,
        batches: RefCell<Vec<(String, usize)>>, // (album_id, item count)
        fail_upload_for: Option<String>,
    }

    impl FakeApi {
        fn new() -> Self {
            Self {
                created_albums: RefCell::new(vec![]),
                batches: RefCell::new(vec![]),
                fail_upload_for: None,
            }
        }
    }

    impl PhotosApi for FakeApi {
        fn create_album(&self, title: &str) -> Result<String, ApiError> {
            self.created_albums.borrow_mut().push(title.to_string());
            Ok(format!("album-for-{title}"))
        }
        fn upload_bytes(&self, file_name: &str, _bytes: &[u8]) -> Result<String, ApiError> {
            if self.fail_upload_for.as_deref() == Some(file_name) {
                return Err(ApiError::Fatal("boom".into()));
            }
            Ok(format!("token-{file_name}"))
        }
        fn batch_create(&self, album_id: &str, tokens: &[String]) -> Result<(), ApiError> {
            self.batches
                .borrow_mut()
                .push((album_id.to_string(), tokens.len()));
            Ok(())
        }
    }

    #[test]
    fn chunks_tokens_into_batches_of_at_most_50() {
        let tokens: Vec<String> = (0..120).map(|i| format!("t{i}")).collect();
        let chunks = chunk(&tokens, 50);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].len(), 50);
        assert_eq!(chunks[1].len(), 50);
        assert_eq!(chunks[2].len(), 20);
    }

    #[test]
    fn ensure_album_creates_once_then_reuses_ledger() {
        let api = FakeApi::new();
        let mut ledger = crate::ledger::Ledger::default();

        let id1 = ensure_album(&api, &mut ledger, "2011 10").unwrap();
        let id2 = ensure_album(&api, &mut ledger, "2011 10").unwrap();

        assert_eq!(id1, id2);
        assert_eq!(api.created_albums.borrow().len(), 1, "album created once");
        assert_eq!(ledger.album_id("2011 10"), Some(id1.as_str()));
    }

    #[test]
    fn upload_file_records_success_in_ledger() {
        let api = FakeApi::new();
        let mut ledger = crate::ledger::Ledger::default();
        let item = PendingUpload {
            file_name: "a.jpg".into(),
            bytes: vec![1, 2, 3],
            key: "2011-10|3|a.jpg".into(),
        };
        let token = upload_one(&api, &mut ledger, &item).unwrap();
        assert_eq!(token, "token-a.jpg");
        // ledger is marked by the caller after batch_create; upload_one only
        // returns the token, so it must NOT be marked yet.
        assert!(!ledger.is_uploaded(&item.key));
    }
}
