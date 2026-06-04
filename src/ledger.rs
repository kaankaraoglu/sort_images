//! Local upload ledger: which `YYYY MM` albums exist in Google Photos and
//! which files have already been uploaded. Persisted as JSON so `--upload`
//! is safely repeatable — only successful uploads are recorded, so a re-run
//! retries exactly the files that failed.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Ledger {
    /// "YYYY MM" -> Google Photos album id.
    albums: HashMap<String, String>,
    /// Keys of files already uploaded (see `upload_key`).
    uploaded: HashSet<String>,
}

/// Stable key for a media file: capture month + byte size + file name. Path is
/// deliberately excluded because the sort moves files between directories.
pub fn upload_key(captured: &str, size: u64, file_name: &str) -> String {
    format!("{captured}|{size}|{file_name}")
}

impl Ledger {
    pub fn is_uploaded(&self, key: &str) -> bool {
        self.uploaded.contains(key)
    }

    pub fn mark_uploaded(&mut self, key: String) {
        self.uploaded.insert(key);
    }

    pub fn album_id(&self, month: &str) -> Option<&str> {
        self.albums.get(month).map(String::as_str)
    }

    pub fn set_album_id(&mut self, month: &str, id: &str) {
        self.albums.insert(month.to_string(), id.to_string());
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|e| e.to_string())
    }

    pub fn from_json(s: &str) -> Result<Self, String> {
        serde_json::from_str(s).map_err(|e| e.to_string())
    }

    /// Load from `path`, returning a fresh empty ledger if the file is absent.
    pub fn load(path: &Path) -> Result<Self, String> {
        match fs::read_to_string(path) {
            Ok(text) => Self::from_json(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Write atomically (write to a temp file, then rename).
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, self.to_json()?).map_err(|e| e.to_string())?;
        fs::rename(&tmp, path).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_combines_date_size_and_name() {
        let k = upload_key("2011-10", 12345, "IMG_1.HEIC");
        assert_eq!(k, "2011-10|12345|IMG_1.HEIC");
    }

    #[test]
    fn records_and_detects_uploaded_files() {
        let mut ledger = Ledger::default();
        let key = upload_key("2011-10", 10, "a.jpg");
        assert!(!ledger.is_uploaded(&key));
        ledger.mark_uploaded(key.clone());
        assert!(ledger.is_uploaded(&key));
    }

    #[test]
    fn remembers_album_ids_per_month() {
        let mut ledger = Ledger::default();
        assert_eq!(ledger.album_id("2011 10"), None);
        ledger.set_album_id("2011 10", "album-abc");
        assert_eq!(ledger.album_id("2011 10"), Some("album-abc"));
    }

    #[test]
    fn round_trips_through_json() {
        let mut ledger = Ledger::default();
        ledger.set_album_id("2026 01", "alb-1");
        ledger.mark_uploaded(upload_key("2026-01", 5, "x.png"));
        let json = ledger.to_json().unwrap();
        let back = Ledger::from_json(&json).unwrap();
        assert_eq!(back.album_id("2026 01"), Some("alb-1"));
        assert!(back.is_uploaded(&upload_key("2026-01", 5, "x.png")));
    }
}
