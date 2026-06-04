//! Sort image and video files into `YYYY-MM` folders based on the month they
//! were taken, optionally uploading them to Google Photos.
//!
//! Usage:
//!     sort_images <folder> [--dry-run] [--recursive] [--upload]

mod config;
mod google;
mod ledger;
mod sort;

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process;

use config::{ledger_path, token_path, AppConfig};
use google::auth::authorize;
use google::photos::{upload_album, PendingUpload, UreqPhotos};
use ledger::Ledger;
use sort::{
    collect_media, file_name, index_images_by_stem, move_file, plan_destination, purge_ds_store,
    unique_dest,
};

struct Config {
    root: PathBuf,
    dry_run: bool,
    recursive: bool,
    upload: bool,
}

/// Holds everything an upload pass needs.
struct Uploader {
    api: UreqPhotos,
    ledger: Ledger,
    ledger_path: PathBuf,
    /// Pending uploads grouped by "YYYY-MM" album title.
    pending: HashMap<String, Vec<PendingUpload>>,
}

fn build_uploader() -> Result<Uploader, String> {
    let app = AppConfig::load()?;
    let token = authorize(&app.google, &token_path()?)?;
    Ok(Uploader {
        api: UreqPhotos::new(token),
        ledger: Ledger::load(&ledger_path()?)?,
        ledger_path: ledger_path()?,
        pending: HashMap::new(),
    })
}

/// Uploading happens only when `--upload` is set and we are NOT in dry-run —
/// this is the single gate that guarantees `--dry-run` performs no network I/O.
fn should_upload(upload: bool, dry_run: bool) -> bool {
    upload && !dry_run
}

fn main() {
    let cfg = match parse_args() {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("{msg}");
            eprintln!("Usage: sort_images <folder> [--dry-run] [--recursive] [--upload]");
            process::exit(2);
        }
    };

    if !cfg.root.is_dir() {
        eprintln!("Error: '{}' is not a directory.", cfg.root.display());
        process::exit(1);
    }

    let mut uploader = if should_upload(cfg.upload, cfg.dry_run) {
        match build_uploader() {
            Ok(u) => Some(u),
            Err(e) => {
                eprintln!("Error: could not initialize Google Photos upload: {e}");
                process::exit(1);
            }
        }
    } else {
        None
    };

    let mut files = Vec::new();
    if let Err(e) = collect_media(&cfg.root, cfg.recursive, &mut files) {
        eprintln!("Error reading directory: {e}");
        process::exit(1);
    }

    let images_by_stem = index_images_by_stem(&files);

    let (mut moved, mut skipped, mut errors) = (0u32, 0u32, 0u32);
    let (mut up_uploaded, mut up_skipped, mut up_failed) = (0u32, 0u32, 0u32);

    for path in files {
        match plan_destination(&path, &images_by_stem) {
            Ok(p) => {
                let month_dir = format!("{}-{:02}", p.year, p.month);
                let bucket = format!("{month_dir}/{}", p.subdir);
                let dest_dir = cfg.root.join(&month_dir).join(p.subdir);
                let dest = unique_dest(&dest_dir, &path);

                if cfg.dry_run {
                    println!(
                        "[dry-run] {} -> {} ({})",
                        file_name(&path),
                        bucket,
                        p.source
                    );
                    if cfg.upload {
                        println!(
                            "[dry-run] would upload {} to album {}-{:02}",
                            file_name(&path),
                            p.year,
                            p.month
                        );
                    }
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

                        if let Some(up) = uploader.as_mut() {
                            let month = format!("{}-{:02}", p.year, p.month);
                            let name = file_name(&dest);
                            let size = fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
                            let captured = format!("{}-{:02}", p.year, p.month);
                            let key = ledger::upload_key(&captured, size, &name);
                            if up.ledger.is_uploaded(&key) {
                                up_skipped += 1;
                            } else {
                                up.pending.entry(month).or_default().push(PendingUpload {
                                    file_name: name,
                                    path: dest.clone(),
                                    key,
                                });
                            }
                        }
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

    if let Some(up) = uploader.as_mut() {
        let mut months: Vec<String> = up.pending.keys().cloned().collect();
        months.sort();
        for month in months {
            let items = up.pending.remove(&month).unwrap_or_default();
            let counts = upload_album(&up.api, &mut up.ledger, &month, &items);
            up_uploaded += counts.uploaded;
            up_failed += counts.failed;
        }
        if let Err(e) = up.ledger.save(&up.ledger_path) {
            eprintln!("  ! could not save upload ledger: {e}");
        }
        println!("Upload: {up_uploaded} uploaded, {up_skipped} skipped, {up_failed} failed.");
    }

    if up_failed > 0 {
        process::exit(1);
    }
}

fn parse_args() -> Result<Config, String> {
    let mut root: Option<PathBuf> = None;
    let mut dry_run = false;
    let mut recursive = false;
    let mut upload = false;

    for arg in env::args().skip(1) {
        match arg.as_str() {
            "--dry-run" => dry_run = true,
            "--recursive" | "-r" => recursive = true,
            "--upload" => upload = true,
            "-h" | "--help" => return Err("Sort images into `YYYY-MM` folders.".to_string()),
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
        upload,
    })
}

#[cfg(test)]
mod tests {
    use super::should_upload;

    #[test]
    fn dry_run_never_uploads_even_with_upload_flag() {
        assert!(
            !should_upload(true, true),
            "dry-run must disable uploads (no network)"
        );
        assert!(should_upload(true, false));
        assert!(!should_upload(false, false));
        assert!(!should_upload(false, true));
    }
}
