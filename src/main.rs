//! Sort image and video files into `YYYY MM` folders based on the month they
//! were taken, optionally uploading them to Google Photos.
//!
//! Usage:
//!     sort_images <folder> [--dry-run] [--recursive]

mod config;
mod google;
mod ledger;
mod sort;

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process;

use sort::{
    collect_media, file_name, index_images_by_stem, move_file, plan_destination, purge_ds_store,
    unique_dest,
};

struct Config {
    root: PathBuf,
    dry_run: bool,
    recursive: bool,
}

fn main() {
    let cfg = match parse_args() {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("{msg}");
            eprintln!("Usage: sort_images <folder> [--dry-run] [--recursive]");
            process::exit(2);
        }
    };

    if !cfg.root.is_dir() {
        eprintln!("Error: '{}' is not a directory.", cfg.root.display());
        process::exit(1);
    }

    let mut files = Vec::new();
    if let Err(e) = collect_media(&cfg.root, cfg.recursive, &mut files) {
        eprintln!("Error reading directory: {e}");
        process::exit(1);
    }

    let images_by_stem = index_images_by_stem(&files);

    let (mut moved, mut skipped, mut errors) = (0u32, 0u32, 0u32);

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

fn parse_args() -> Result<Config, String> {
    let mut root: Option<PathBuf> = None;
    let mut dry_run = false;
    let mut recursive = false;

    for arg in env::args().skip(1) {
        match arg.as_str() {
            "--dry-run" => dry_run = true,
            "--recursive" | "-r" => recursive = true,
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
    })
}
