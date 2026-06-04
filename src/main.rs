//! Sort image and video files into `YYYY MM` folders based on the month they
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
use google::photos::{
    chunk, ensure_album, upload_one, PendingUpload, PhotosApi, UreqPhotos, BATCH_LIMIT,
};
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
    /// Pending uploads grouped by "YYYY MM" album title.
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

    let mut uploader = if cfg.upload && !cfg.dry_run {
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
                let month_dir = format!("{} {:02}", p.year, p.month);
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
                            "[dry-run] would upload {} to album {} {:02}",
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
                            let month = format!("{} {:02}", p.year, p.month);
                            let name = file_name(&dest);
                            let size = fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
                            let captured = format!("{}-{:02}", p.year, p.month);
                            let key = ledger::upload_key(&captured, size, &name);
                            if up.ledger.is_uploaded(&key) {
                                up_skipped += 1;
                            } else {
                                match fs::read(&dest) {
                                    Ok(bytes) => {
                                        up.pending.entry(month).or_default().push(PendingUpload {
                                            file_name: name,
                                            bytes,
                                            key,
                                        })
                                    }
                                    Err(e) => {
                                        eprintln!(
                                            "  ! could not read {} for upload: {e}",
                                            dest.display()
                                        );
                                        up_failed += 1;
                                    }
                                }
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
        // Sort album titles for deterministic, chronological output.
        let mut months: Vec<String> = up.pending.keys().cloned().collect();
        months.sort();
        for month in months {
            let items = up.pending.remove(&month).unwrap_or_default();
            let album_id = match ensure_album(&up.api, &mut up.ledger, &month) {
                Ok(id) => id,
                Err(e) => {
                    eprintln!("  ! could not create/find album {month}: {e}");
                    up_failed += items.len() as u32;
                    continue;
                }
            };

            // Upload bytes, collecting (token, key) for successes.
            let mut uploaded: Vec<(String, String)> = Vec::new();
            for item in &items {
                match upload_one(&up.api, &mut up.ledger, item) {
                    Ok(token) => uploaded.push((token, item.key.clone())),
                    Err(e) => {
                        eprintln!("  ! upload failed for {}: {e}", item.file_name);
                        up_failed += 1;
                    }
                }
            }

            // Attach in batches of <= BATCH_LIMIT; mark ledger only on success.
            for batch in chunk(&uploaded, BATCH_LIMIT) {
                let tokens: Vec<String> = batch.iter().map(|(t, _)| t.clone()).collect();
                match up.api.batch_create(&album_id, &tokens) {
                    Ok(()) => {
                        for (_, key) in &batch {
                            up.ledger.mark_uploaded(key.clone());
                            up_uploaded += 1;
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "  ! attaching {} item(s) to {month} failed: {e}",
                            tokens.len()
                        );
                        up_failed += tokens.len() as u32;
                    }
                }
            }
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
            "-h" | "--help" => return Err("Sort images into `YYYY MM` folders.".to_string()),
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
