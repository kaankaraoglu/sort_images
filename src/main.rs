//! Sort image files into `YYYY/Qn` folders based on the quarter they were taken.
//!
//! For each image the capture date is read from EXIF (DateTimeOriginal, then
//! DateTimeDigitized, then DateTime). If no EXIF date is found, the file's last
//! modified time is used as a fallback. Files are then *moved* into a
//! subfolder named e.g. `2026/Q1` or `2011/Q4`.
//!
//! Usage:
//!     sort_images <folder> [--dry-run] [--recursive]
//!
//!     <folder>      Directory containing the images to sort.
//!     --dry-run     Print what would happen without moving any files.
//!     --recursive   Also descend into subdirectories (skips dirs that look
//!                   like already-created YYYY/Qn buckets).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

use chrono::{DateTime, Datelike, Local};

const IMAGE_EXTS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "bmp", "tif", "tiff", "webp", "heic", "heif",
    "cr2", "cr3", "nef", "arw", "dng", "raf", "orf", "rw2", "sr2",
];

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
    if let Err(e) = collect_images(&cfg.root, cfg.recursive, &mut files) {
        eprintln!("Error reading directory: {e}");
        process::exit(1);
    }

    let (mut moved, mut skipped, mut errors) = (0u32, 0u32, 0u32);

    for path in files {
        match quarter_dir_for(&path) {
            Ok((year, quarter, source)) => {
                let dest_dir = cfg.root.join(year.to_string()).join(format!("Q{quarter}"));
                let dest = unique_dest(&dest_dir, &path);

                if cfg.dry_run {
                    println!(
                        "[dry-run] {} -> {}/Q{} ({})",
                        file_name(&path),
                        year,
                        quarter,
                        source
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
                        println!("{} -> {}/Q{} ({})", file_name(&path), year, quarter, source);
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
        if cfg.dry_run { " (dry run — nothing changed)" } else { "" }
    );
}

fn parse_args() -> Result<Config, String> {
    let mut root: Option<PathBuf> = None;
    let mut dry_run = false;
    let mut recursive = false;

    for arg in env::args().skip(1) {
        match arg.as_str() {
            "--dry-run" => dry_run = true,
            "--recursive" | "-r" => recursive = true,
            "-h" | "--help" => return Err("Sort images into YYYY/Qn folders.".to_string()),
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
    Ok(Config { root, dry_run, recursive })
}

/// Recursively (or not) gather image files under `dir`.
fn collect_images(dir: &Path, recursive: bool, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            if recursive && !looks_like_bucket(&path) {
                collect_images(&path, recursive, out)?;
            }
        } else if file_type.is_file() && is_image(&path) {
            out.push(path);
        }
    }
    Ok(())
}

/// Avoid descending into folders we (or a previous run) created, e.g. `Q1`.
fn looks_like_bucket(path: &Path) -> bool {
    match path.file_name().and_then(|n| n.to_str()) {
        Some(name) => {
            name.len() == 2
                && name.starts_with('Q')
                && name[1..].chars().all(|c| c.is_ascii_digit())
        }
        None => false,
    }
}

fn is_image(path: &Path) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => {
            let ext = ext.to_ascii_lowercase();
            IMAGE_EXTS.contains(&ext.as_str())
        }
        None => false,
    }
}

/// Return (year, quarter, source-of-date) for an image.
fn quarter_dir_for(path: &Path) -> Result<(i32, u32, &'static str), String> {
    if let Some((year, month)) = exif_year_month(path) {
        return Ok((year, quarter_of(month), "exif"));
    }
    let (year, month) = mtime_year_month(path)?;
    Ok((year, quarter_of(month), "mtime"))
}

fn quarter_of(month: u32) -> u32 {
    ((month - 1) / 3) + 1
}

/// Try to read the capture date from EXIF metadata.
fn exif_year_month(path: &Path) -> Option<(i32, u32)> {
    let file = fs::File::open(path).ok()?;
    let mut reader = std::io::BufReader::new(file);
    let exif = exif::Reader::new()
        .read_from_container(&mut reader)
        .ok()?;

    let tags = [
        exif::Tag::DateTimeOriginal,
        exif::Tag::DateTimeDigitized,
        exif::Tag::DateTime,
    ];

    for tag in tags {
        if let Some(field) = exif.get_field(tag, exif::In::PRIMARY) {
            let value = field.display_value().to_string();
            if let Some(ym) = parse_exif_datetime(&value) {
                return Some(ym);
            }
        }
    }
    None
}

/// EXIF datetimes look like "2011:10:25 14:30:00" (some libraries surround the
/// value with quotes, so we strip any non-digit padding first).
fn parse_exif_datetime(s: &str) -> Option<(i32, u32)> {
    let cleaned = s.trim().trim_matches('"').trim();
    let date_part = cleaned.split_whitespace().next()?;
    let mut parts = date_part.split([':', '-']);
    let year: i32 = parts.next()?.trim().parse().ok()?;
    let month: u32 = parts.next()?.trim().parse().ok()?;
    if (1..=12).contains(&month) && year > 0 {
        Some((year, month))
    } else {
        None
    }
}

/// Fall back to the filesystem's last-modified time.
fn mtime_year_month(path: &Path) -> Result<(i32, u32), String> {
    let meta = fs::metadata(path).map_err(|e| e.to_string())?;
    let modified = meta.modified().map_err(|e| e.to_string())?;
    let dt: DateTime<Local> = modified.into();
    Ok((dt.year(), dt.month()))
}

/// Pick a destination path inside `dest_dir`, appending " (n)" on collision.
fn unique_dest(dest_dir: &Path, src: &Path) -> PathBuf {
    let name = src.file_name().unwrap_or_default();
    let candidate = dest_dir.join(name);
    if !candidate.exists() {
        return candidate;
    }

    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let ext = src.extension().and_then(|s| s.to_str());

    for n in 1.. {
        let new_name = match ext {
            Some(ext) => format!("{stem} ({n}).{ext}"),
            None => format!("{stem} ({n})"),
        };
        let candidate = dest_dir.join(new_name);
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
}

/// Move a file, falling back to copy+delete across filesystems.
fn move_file(src: &Path, dest: &Path) -> std::io::Result<()> {
    match fs::rename(src, dest) {
        Ok(()) => Ok(()),
        Err(_) => {
            fs::copy(src, dest)?;
            fs::remove_file(src)?;
            Ok(())
        }
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}
