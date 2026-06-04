//! Google Photos Library API client. The network surface is expressed as the
//! `PhotosApi` trait so upload orchestration can be tested with a fake; the
//! `UreqPhotos` implementation talks to the real API over HTTPS.

use std::path::PathBuf;

use crate::ledger::Ledger;

/// Max media items per `mediaItems:batchCreate` call (Google's documented limit).
pub const BATCH_LIMIT: usize = 50;

const UPLOAD_URL: &str = "https://photoslibrary.googleapis.com/v1/uploads";
const ALBUMS_URL: &str = "https://photoslibrary.googleapis.com/v1/albums";
const BATCH_URL: &str = "https://photoslibrary.googleapis.com/v1/mediaItems:batchCreate";

/// A file queued for upload: its name, on-disk path (read lazily at upload
/// time to cap peak memory at one file), and ledger key.
pub struct PendingUpload {
    pub file_name: String,
    pub path: PathBuf,
    pub key: String,
}

/// Outcome for one media item in a batchCreate response.
pub struct ItemOutcome {
    pub token: String,
    pub ok: bool,
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
    fn batch_create(&self, album_id: &str, tokens: &[String])
        -> Result<Vec<ItemOutcome>, ApiError>;
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

/// Map a batchCreate response to per-token outcomes. An item is OK when its
/// result echoes the `uploadToken` and carries a `mediaItem` (Google sets a
/// non-zero `status` and omits `mediaItem` on failure). Any token without a
/// matching successful result is reported as failed.
fn parse_batch_results(tokens: &[String], body: &serde_json::Value) -> Vec<ItemOutcome> {
    let results = body.get("newMediaItemResults").and_then(|r| r.as_array());
    tokens
        .iter()
        .map(|t| {
            let ok = results.is_some_and(|arr| {
                arr.iter().any(|r| {
                    r.get("uploadToken").and_then(|u| u.as_str()) == Some(t.as_str())
                        && r.get("mediaItem").is_some()
                })
            });
            ItemOutcome {
                token: t.clone(),
                ok,
            }
        })
        .collect()
}

/// Success/failure tallies for one album's upload pass.
pub struct UploadCounts {
    pub uploaded: u32,
    pub failed: u32,
}

/// Upload all `items` into the `album_title` album: ensure the album exists,
/// read+upload each file's bytes lazily (one file in memory at a time), attach
/// in `BATCH_LIMIT`-sized batches, and mark the ledger only for items Google
/// accepted. Per-file and per-item failures are logged and counted, never
/// fatal — so a re-run retries exactly the files that did not succeed.
pub fn upload_album<A: PhotosApi>(
    api: &A,
    ledger: &mut Ledger,
    album_title: &str,
    items: &[PendingUpload],
) -> UploadCounts {
    let mut counts = UploadCounts {
        uploaded: 0,
        failed: 0,
    };
    if items.is_empty() {
        return counts;
    }
    let album_id = match ensure_album(api, ledger, album_title) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("  ! could not create/find album {album_title}: {e}");
            counts.failed += items.len() as u32;
            return counts;
        }
    };

    // Upload bytes (read lazily), collecting (token, key) for successes.
    let mut uploaded: Vec<(String, String)> = Vec::new();
    for item in items {
        let bytes = match std::fs::read(&item.path) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("  ! could not read {} for upload: {e}", item.path.display());
                counts.failed += 1;
                continue;
            }
        };
        match api.upload_bytes(&item.file_name, &bytes) {
            Ok(token) => uploaded.push((token, item.key.clone())),
            Err(e) => {
                eprintln!("  ! upload failed for {}: {e}", item.file_name);
                counts.failed += 1;
            }
        }
    }

    // Attach in batches; mark the ledger only for items Google accepted.
    for batch in chunk(&uploaded, BATCH_LIMIT) {
        let tokens: Vec<String> = batch.iter().map(|(t, _)| t.clone()).collect();
        match api.batch_create(&album_id, &tokens) {
            Ok(outcomes) => {
                for (token, key) in &batch {
                    if outcomes.iter().any(|o| &o.token == token && o.ok) {
                        ledger.mark_uploaded(key.clone());
                        counts.uploaded += 1;
                    } else {
                        eprintln!("  ! Google rejected upload for {key}");
                        counts.failed += 1;
                    }
                }
            }
            Err(e) => {
                eprintln!(
                    "  ! attaching {} item(s) to {album_title} failed: {e}",
                    tokens.len()
                );
                counts.failed += tokens.len() as u32;
            }
        }
    }
    counts
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

    fn batch_create(
        &self,
        album_id: &str,
        tokens: &[String],
    ) -> Result<Vec<ItemOutcome>, ApiError> {
        let new_items: Vec<_> = tokens
            .iter()
            .map(|t| json!({ "simpleMediaItem": { "uploadToken": t } }))
            .collect();
        with_retry(|| {
            let resp = ureq::post(BATCH_URL)
                .set("Authorization", &self.bearer())
                .send_json(json!({ "albumId": album_id, "newMediaItems": new_items }))
                .map_err(Self::classify)?;
            let v: serde_json::Value = resp
                .into_json()
                .map_err(|e| ApiError::Fatal(e.to_string()))?;
            Ok(parse_batch_results(tokens, &v))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::fs;

    /// Records calls and returns canned ids so we can assert orchestration.
    struct FakeApi {
        created_albums: RefCell<Vec<String>>,
        fail_create: bool,
        fail_upload_for: Option<String>,
        fail_batch_token: Option<String>, // token Google "rejects"
    }

    impl FakeApi {
        fn new() -> Self {
            Self {
                created_albums: RefCell::new(vec![]),
                fail_create: false,
                fail_upload_for: None,
                fail_batch_token: None,
            }
        }
    }

    impl PhotosApi for FakeApi {
        fn create_album(&self, title: &str) -> Result<String, ApiError> {
            if self.fail_create {
                return Err(ApiError::Fatal("create failed".into()));
            }
            self.created_albums.borrow_mut().push(title.to_string());
            Ok(format!("album-for-{title}"))
        }
        fn upload_bytes(&self, file_name: &str, _bytes: &[u8]) -> Result<String, ApiError> {
            if self.fail_upload_for.as_deref() == Some(file_name) {
                return Err(ApiError::Fatal("boom".into()));
            }
            Ok(format!("token-{file_name}"))
        }
        fn batch_create(
            &self,
            _album_id: &str,
            tokens: &[String],
        ) -> Result<Vec<ItemOutcome>, ApiError> {
            Ok(tokens
                .iter()
                .map(|t| ItemOutcome {
                    token: t.clone(),
                    ok: self.fail_batch_token.as_deref() != Some(t.as_str()),
                })
                .collect())
        }
    }

    fn tmp_file(tag: &str, n: usize) -> std::path::PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("sort_images-photos-{tag}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("f{n}.jpg"));
        fs::write(&path, b"data").unwrap();
        path
    }

    fn pending(tag: &str, n: usize, name: &str) -> PendingUpload {
        PendingUpload {
            file_name: name.to_string(),
            path: tmp_file(tag, n),
            key: format!("k-{name}"),
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
    fn upload_album_marks_only_successful_items() {
        let api = FakeApi::new();
        let mut ledger = crate::ledger::Ledger::default();
        let items = vec![pending("ok", 0, "a.jpg"), pending("ok", 1, "b.jpg")];
        let counts = upload_album(&api, &mut ledger, "2011 10", &items);
        assert_eq!(counts.uploaded, 2);
        assert_eq!(counts.failed, 0);
        assert!(ledger.is_uploaded("k-a.jpg"));
        assert!(ledger.is_uploaded("k-b.jpg"));
    }

    #[test]
    fn upload_album_counts_upload_failures_and_does_not_mark_them() {
        let mut api = FakeApi::new();
        api.fail_upload_for = Some("b.jpg".into());
        let mut ledger = crate::ledger::Ledger::default();
        let items = vec![pending("uf", 0, "a.jpg"), pending("uf", 1, "b.jpg")];
        let counts = upload_album(&api, &mut ledger, "2011 10", &items);
        assert_eq!(counts.uploaded, 1);
        assert_eq!(counts.failed, 1);
        assert!(ledger.is_uploaded("k-a.jpg"));
        assert!(!ledger.is_uploaded("k-b.jpg"));
    }

    #[test]
    fn upload_album_honors_per_item_batch_rejection() {
        let mut api = FakeApi::new();
        api.fail_batch_token = Some("token-b.jpg".into());
        let mut ledger = crate::ledger::Ledger::default();
        let items = vec![pending("br", 0, "a.jpg"), pending("br", 1, "b.jpg")];
        let counts = upload_album(&api, &mut ledger, "2011 10", &items);
        assert_eq!(counts.uploaded, 1);
        assert_eq!(counts.failed, 1);
        assert!(ledger.is_uploaded("k-a.jpg"));
        assert!(
            !ledger.is_uploaded("k-b.jpg"),
            "rejected item must remain unmarked for retry"
        );
    }

    #[test]
    fn upload_album_fails_all_when_album_creation_fails() {
        let mut api = FakeApi::new();
        api.fail_create = true;
        let mut ledger = crate::ledger::Ledger::default();
        let items = vec![pending("ac", 0, "a.jpg")];
        let counts = upload_album(&api, &mut ledger, "2011 10", &items);
        assert_eq!(counts.uploaded, 0);
        assert_eq!(counts.failed, 1);
    }

    #[test]
    fn parse_batch_results_flags_missing_and_rejected_items() {
        let tokens = vec!["t1".to_string(), "t2".to_string(), "t3".to_string()];
        let body = serde_json::json!({
            "newMediaItemResults": [
                { "uploadToken": "t1", "mediaItem": { "id": "x" } },
                { "uploadToken": "t2", "status": { "code": 3, "message": "bad" } }
                // t3 absent entirely
            ]
        });
        let outcomes = parse_batch_results(&tokens, &body);
        assert!(outcomes.iter().find(|o| o.token == "t1").unwrap().ok);
        assert!(!outcomes.iter().find(|o| o.token == "t2").unwrap().ok);
        assert!(!outcomes.iter().find(|o| o.token == "t3").unwrap().ok);
    }
}
