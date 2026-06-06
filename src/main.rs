//! Sort image and video files into date folders based on the month they were
//! taken. The folder layout is configurable via `config.toml` and CLI flags.
//!
//! Usage:
//!     sort_images <folder> [--dry-run] [--recursive]
//!                          [--format <TEMPLATE>] [--split | --no-split]

mod config;
mod sort;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

use config::{load_file_config, resolve_settings, Settings};
use sort::{
    collect_media, file_name, index_images_by_stem, move_file, plan_destination, purge_ds_store,
    unique_dest,
};

const USAGE: &str = "Usage: sort_images <folder> [--dry-run] [--recursive] \
[--format <TEMPLATE>] [--split | --no-split]";

/// The config file looked up in the current working directory.
const CONFIG_FILE: &str = "config.toml";

struct Config {
    root: PathBuf,
    dry_run: bool,
    recursive: bool,
    /// CLI overrides for the folder layout; `None` means "fall back to
    /// config.toml, then the built-in default".
    format: Option<String>,
    split: Option<bool>,
}

fn main() {
    let cfg = match parse_args(env::args().skip(1)) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("{msg}");
            eprintln!("{USAGE}");
            process::exit(2);
        }
    };

    if !cfg.root.is_dir() {
        eprintln!("Error: '{}' is not a directory.", cfg.root.display());
        process::exit(1);
    }

    let settings = match load_settings(&cfg) {
        Ok(s) => s,
        Err(msg) => {
            eprintln!("Error: {msg}");
            process::exit(1);
        }
    };

    let mut files = Vec::new();
    if let Err(e) = collect_media(&cfg.root, cfg.recursive, &settings.format, &mut files) {
        eprintln!("Error reading directory: {e}");
        process::exit(1);
    }

    let images_by_stem = index_images_by_stem(&files);

    let (mut moved, mut skipped, mut errors) = (0u32, 0u32, 0u32);

    for path in files {
        match plan_destination(&path, &images_by_stem) {
            Ok(p) => {
                let date_dir = settings.format.render(p.year, p.month);
                let (bucket, dest_dir) = if settings.split {
                    (
                        format!("{date_dir}/{}", p.subdir),
                        cfg.root.join(&date_dir).join(p.subdir),
                    )
                } else {
                    (date_dir.clone(), cfg.root.join(&date_dir))
                };
                let dest = unique_dest(&dest_dir, &path);

                if cfg.dry_run {
                    println!(
                        "[dry-run] {} -> {} ({})",
                        file_name(&path),
                        bucket,
                        p.source
                    );
                    moved += 1;
                    continue;
                }

                if let Err(e) = fs::create_dir_all(&dest_dir) {
                    eprintln!("  ! could not create {}: {e}", dest_dir.display());
                    errors += 1;
                    continue;
                }

                match move_file(&path, &dest) {
                    Ok(()) => {
                        println!("{} -> {} ({})", file_name(&path), bucket, p.source);
                        moved += 1;
                    }
                    Err(e) => {
                        eprintln!("  ! failed to move {}: {e}", path.display());
                        errors += 1;
                    }
                }
            }
            Err(e) => {
                eprintln!("  ! skipping {}: {e}", path.display());
                skipped += 1;
            }
        }
    }

    println!(
        "\nDone. {} moved, {} skipped, {} errors.{}",
        moved,
        skipped,
        errors,
        if cfg.dry_run {
            " (dry run — nothing changed)"
        } else {
            ""
        }
    );

    match purge_ds_store(&cfg.root, cfg.dry_run) {
        Ok(n) if cfg.dry_run => println!("[dry-run] would remove {n} .DS_Store file(s)."),
        Ok(n) => println!("Removed {n} .DS_Store file(s)."),
        Err(e) => eprintln!("  ! .DS_Store cleanup failed: {e}"),
    }
}

/// Load `config.toml` (if any) and resolve it together with the CLI overrides.
fn load_settings(cfg: &Config) -> Result<Settings, String> {
    let file = load_file_config(Path::new(CONFIG_FILE))?;
    resolve_settings(file, cfg.format.clone(), cfg.split)
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Config, String> {
    let mut root: Option<PathBuf> = None;
    let mut dry_run = false;
    let mut recursive = false;
    let mut format: Option<String> = None;
    let mut split: Option<bool> = None;

    let mut args = args;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dry-run" => dry_run = true,
            "--recursive" | "-r" => recursive = true,
            "--format" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--format requires a template argument.".to_string())?;
                format = Some(value);
            }
            "--split" => {
                if split == Some(false) {
                    return Err("--split and --no-split are mutually exclusive.".to_string());
                }
                split = Some(true);
            }
            "--no-split" => {
                if split == Some(true) {
                    return Err("--split and --no-split are mutually exclusive.".to_string());
                }
                split = Some(false);
            }
            "-h" | "--help" => {
                return Err("Sort images into configurable date folders.".to_string())
            }
            other if other.starts_with('-') => {
                return Err(format!("Unknown option: {other}"));
            }
            other => {
                if root.is_some() {
                    return Err("Only one folder may be given.".to_string());
                }
                root = Some(PathBuf::from(other));
            }
        }
    }

    let root = root.ok_or_else(|| "Missing folder argument.".to_string())?;
    Ok(Config {
        root,
        dry_run,
        recursive,
        format,
        split,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Config, String> {
        parse_args(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_root_and_defaults_overrides_to_none() {
        let cfg = parse(&["/pics"]).unwrap();
        assert_eq!(cfg.root, PathBuf::from("/pics"));
        assert!(!cfg.dry_run);
        assert!(!cfg.recursive);
        assert_eq!(cfg.format, None);
        assert_eq!(cfg.split, None);
    }

    #[test]
    fn parses_format_value() {
        let cfg = parse(&["/pics", "--format", "{year}/{month}"]).unwrap();
        assert_eq!(cfg.format.as_deref(), Some("{year}/{month}"));
    }

    #[test]
    fn format_without_value_is_error() {
        assert!(parse(&["/pics", "--format"]).is_err());
    }

    #[test]
    fn split_and_no_split_set_the_flag() {
        assert_eq!(parse(&["/pics", "--split"]).unwrap().split, Some(true));
        assert_eq!(parse(&["/pics", "--no-split"]).unwrap().split, Some(false));
    }

    #[test]
    fn split_and_no_split_together_is_error() {
        assert!(parse(&["/pics", "--split", "--no-split"]).is_err());
        assert!(parse(&["/pics", "--no-split", "--split"]).is_err());
    }
}
