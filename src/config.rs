//! Application configuration: OAuth client credentials read from
//! `~/.config/sort_images/config.toml`, plus the state directory paths used
//! for the token cache and upload ledger.

use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct AppConfig {
    pub google: GoogleConfig,
}

#[derive(Debug, Deserialize)]
pub struct GoogleConfig {
    pub client_id: String,
    pub client_secret: String,
}

impl AppConfig {
    /// Parse config from a TOML string. Separated from file IO so it is
    /// unit-testable without touching the filesystem.
    pub fn from_toml_str(s: &str) -> Result<Self, String> {
        toml::from_str(s).map_err(|e| e.to_string())
    }

    /// Load config from the default path (`config_path()`).
    pub fn load() -> Result<Self, String> {
        let path = config_path()?;
        let text = fs::read_to_string(&path).map_err(|e| {
            format!(
                "could not read config at {}: {e}\n\
                 Create it with a [google] section (client_id, client_secret).",
                path.display()
            )
        })?;
        Self::from_toml_str(&text)
    }
}

/// `~/.config/sort_images`, honoring `$XDG_CONFIG_HOME` then `$HOME`.
pub fn state_dir() -> Result<PathBuf, String> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return Ok(PathBuf::from(xdg).join("sort_images"));
        }
    }
    let home = std::env::var("HOME").map_err(|_| "HOME is not set".to_string())?;
    Ok(PathBuf::from(home).join(".config").join("sort_images"))
}

pub fn config_path() -> Result<PathBuf, String> {
    Ok(state_dir()?.join("config.toml"))
}

pub fn token_path() -> Result<PathBuf, String> {
    Ok(state_dir()?.join("token.json"))
}

pub fn ledger_path() -> Result<PathBuf, String> {
    Ok(state_dir()?.join("ledger.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_google_credentials() {
        let toml = r#"
            [google]
            client_id = "abc.apps.googleusercontent.com"
            client_secret = "secret123"
        "#;
        let cfg = AppConfig::from_toml_str(toml).expect("should parse");
        assert_eq!(cfg.google.client_id, "abc.apps.googleusercontent.com");
        assert_eq!(cfg.google.client_secret, "secret123");
    }

    #[test]
    fn missing_google_section_is_an_error() {
        let err = AppConfig::from_toml_str("").unwrap_err();
        assert!(
            err.contains("google"),
            "error should mention the missing section: {err}"
        );
    }
}
