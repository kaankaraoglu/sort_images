//! Loading and resolving configuration from `config.toml` and CLI overrides
//! into the final [`Settings`] used to lay out destination folders.

use std::path::Path;

use serde::Deserialize;

use crate::sort::FolderFormat;

/// The resolved, ready-to-use configuration.
pub struct Settings {
    pub format: FolderFormat,
    pub split: bool,
}

/// The raw contents of `config.toml`. Every field is optional; unknown keys are
/// rejected so a typo can't silently fall back to defaults.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileConfig {
    pub format: Option<String>,
    pub split: Option<bool>,
}

/// Load and parse `config.toml` at `path`. Returns `Ok(None)` when the file is
/// absent (defaults apply), `Ok(Some(_))` when it parses, and `Err` when it is
/// unreadable, malformed, or has unknown keys.
pub fn load_file_config(path: &Path) -> Result<Option<FileConfig>, String> {
    let contents = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("Could not read {}: {e}", path.display())),
    };
    toml::from_str(&contents)
        .map(Some)
        .map_err(|e| format!("Invalid {}: {e}", path.display()))
}

/// Resolve the final settings from the optional file config and optional CLI
/// overrides. Precedence (low → high): built-in defaults, `config.toml`, CLI.
pub fn resolve_settings(
    file: Option<FileConfig>,
    cli_format: Option<String>,
    cli_split: Option<bool>,
) -> Result<Settings, String> {
    let format_str = cli_format
        .or_else(|| file.as_ref().and_then(|f| f.format.clone()))
        .unwrap_or_else(|| FolderFormat::default_template().to_string());

    let split = cli_split
        .or_else(|| file.and_then(|f| f.split))
        .unwrap_or(true);

    let format = FolderFormat::parse(&format_str)?;
    Ok(Settings { format, split })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_nothing_provided() {
        let settings = resolve_settings(None, None, None).unwrap();
        assert_eq!(settings.format.render(2026, 1), "2026-01");
        assert!(settings.split);
    }

    #[test]
    fn file_config_supplies_format_and_split() {
        let file = FileConfig {
            format: Some("{year}/{month}".to_string()),
            split: Some(false),
        };
        let settings = resolve_settings(Some(file), None, None).unwrap();
        assert_eq!(settings.format.render(2026, 1), "2026/01");
        assert!(!settings.split);
    }

    #[test]
    fn cli_format_overrides_file() {
        let file = FileConfig {
            format: Some("{year}/{month}".to_string()),
            split: None,
        };
        let settings = resolve_settings(Some(file), Some("{year}".to_string()), None).unwrap();
        assert_eq!(settings.format.render(2026, 1), "2026");
    }

    #[test]
    fn cli_split_overrides_file_both_directions() {
        let file_on = FileConfig {
            format: None,
            split: Some(true),
        };
        let off = resolve_settings(Some(file_on), None, Some(false)).unwrap();
        assert!(!off.split);

        let file_off = FileConfig {
            format: None,
            split: Some(false),
        };
        let on = resolve_settings(Some(file_off), None, Some(true)).unwrap();
        assert!(on.split);
    }

    #[test]
    fn invalid_format_is_an_error() {
        assert!(resolve_settings(None, Some("{day}".to_string()), None).is_err());
    }

    fn unique_tmp_file(tag: &str) -> std::path::PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("sort_images-cfg-{tag}-{nanos}.toml"))
    }

    #[test]
    fn missing_file_is_ok_none() {
        let path = unique_tmp_file("missing");
        assert!(load_file_config(&path).unwrap().is_none());
    }

    #[test]
    fn reads_valid_file() {
        let path = unique_tmp_file("valid");
        std::fs::write(&path, "format = \"{year}/{month}\"\nsplit = false\n").unwrap();

        let cfg = load_file_config(&path).unwrap().unwrap();
        assert_eq!(cfg.format.as_deref(), Some("{year}/{month}"));
        assert_eq!(cfg.split, Some(false));

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn unknown_key_is_rejected() {
        let path = unique_tmp_file("unknown");
        std::fs::write(&path, "frmat = \"{year}\"\n").unwrap();

        assert!(load_file_config(&path).is_err());

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn malformed_toml_is_rejected() {
        let path = unique_tmp_file("malformed");
        std::fs::write(&path, "format = \n").unwrap();

        assert!(load_file_config(&path).is_err());

        std::fs::remove_file(&path).unwrap();
    }
}
