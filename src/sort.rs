//! Sorting media into `YYYY MM` buckets: media discovery, capture-date
//! extraction (EXIF / mvhd / mtime), Live Photo pairing, and moving files.

use std::collections::HashMap;
use std::fs;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Datelike, Local, TimeZone, Utc};

const IMAGE_EXTS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "bmp", "tif", "tiff", "webp", "heic", "heif", "cr2", "cr3", "nef",
    "arw", "dng", "raf", "orf", "rw2", "sr2",
];

/// Video container extensions. Capture dates are read from the `mvhd` atom for
/// the QuickTime/ISO-BMFF family (mov, mp4, m4v, 3gp); other containers fall
/// back to the file's last-modified time.
const VIDEO_EXTS: &[&str] = &[
    "mov", "mp4", "m4v", "3gp", "3g2", "qt", "avi", "mkv", "webm", "mts", "m2ts",
];

/// Seconds between the QuickTime/Mac epoch (1904-01-01) and the Unix epoch
/// (1970-01-01). `mvhd` creation times are counted from the Mac epoch.
const MAC_EPOCH_OFFSET_SECS: i64 = 2_082_844_800;

/// Where a single file should be filed: a `YYYY` + month bucket and the
/// `photos`/`videos` subfolder within it, plus where the date came from.
pub struct Placement {
    pub year: i32,
    pub month: u32,
    pub subdir: &'static str,
    pub source: &'static str,
}

/// Recursively (or not) gather image and video files under `dir`.
pub fn collect_media(dir: &Path, recursive: bool, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            if recursive && !looks_like_bucket(&path) {
                collect_media(&path, recursive, out)?;
            }
        } else if file_type.is_file() && is_media(&path) {
            out.push(path);
        }
    }
    Ok(())
}

/// Recursively delete `.DS_Store` files under `dir`, returning the number
/// removed. In `dry_run` mode nothing is deleted but the count of files that
/// would be removed is still returned. Individual failures are logged and
/// skipped rather than aborting the whole sweep.
pub fn purge_ds_store(dir: &Path, dry_run: bool) -> std::io::Result<u32> {
    let mut removed = 0;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            removed += purge_ds_store(&path, dry_run)?;
        } else if file_type.is_file()
            && path.file_name().and_then(|n| n.to_str()) == Some(".DS_Store")
        {
            if dry_run {
                removed += 1;
            } else {
                match fs::remove_file(&path) {
                    Ok(()) => removed += 1,
                    Err(e) => eprintln!("  ! could not remove {}: {e}", path.display()),
                }
            }
        }
    }
    Ok(removed)
}

/// Avoid descending into folders we (or a previous run) created, e.g. `2019 10`.
fn looks_like_bucket(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    match name.split_once(' ') {
        Some((year, month)) => {
            !year.is_empty() && year.chars().all(|c| c.is_ascii_digit()) && is_month_label(month)
        }
        None => false,
    }
}

/// True if `s` is a zero-padded two-digit month (`01`–`12`), as used in the
/// `YYYY MM` bucket names this tool creates.
fn is_month_label(s: &str) -> bool {
    s.len() == 2 && matches!(s.parse::<u32>(), Ok(1..=12))
}

/// True if `path` has an extension we know how to sort (image or video).
fn is_media(path: &Path) -> bool {
    is_image(path) || is_video(path)
}

/// The subfolder a file belongs in within its month bucket: videos go under
/// `videos`, everything else (images) under `photos`.
pub fn media_subdir(path: &Path) -> &'static str {
    if is_video(path) {
        "videos"
    } else {
        "photos"
    }
}

fn is_image(path: &Path) -> bool {
    has_ext_in(path, IMAGE_EXTS)
}

pub fn is_video(path: &Path) -> bool {
    has_ext_in(path, VIDEO_EXTS)
}

fn has_ext_in(path: &Path, exts: &[&str]) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => {
            let ext = ext.to_ascii_lowercase();
            exts.contains(&ext.as_str())
        }
        None => false,
    }
}

/// Index every image by `(parent directory, lowercased file stem)` so a video
/// can be matched to a same-named still (e.g. an Apple Live Photo's `.heic`
/// next to its `.mov`).
pub fn index_images_by_stem(files: &[PathBuf]) -> HashMap<(PathBuf, String), PathBuf> {
    let mut index = HashMap::new();
    for file in files {
        if !is_image(file) {
            continue;
        }
        if let (Some(dir), Some(stem)) = (file.parent(), file.file_stem().and_then(|s| s.to_str()))
        {
            index.insert((dir.to_path_buf(), stem.to_ascii_lowercase()), file.clone());
        }
    }
    index
}

/// If `path` is the motion component of a Live Photo — a video sitting next to
/// an image with the same file stem — return that still's path.
fn live_photo_still<'a>(
    path: &Path,
    images_by_stem: &'a HashMap<(PathBuf, String), PathBuf>,
) -> Option<&'a PathBuf> {
    if !is_video(path) {
        return None;
    }
    let dir = path.parent()?.to_path_buf();
    let stem = path.file_stem()?.to_str()?.to_ascii_lowercase();
    images_by_stem.get(&(dir, stem))
}

/// Decide the destination bucket and subfolder for a file. The motion half of a
/// Live Photo is filed as a photo, in the same month as its still, so the pair
/// stays together.
pub fn plan_destination(
    path: &Path,
    images_by_stem: &HashMap<(PathBuf, String), PathBuf>,
) -> Result<Placement, String> {
    if let Some(still) = live_photo_still(path, images_by_stem) {
        let (year, month, _) = month_dir_for(still)?;
        return Ok(Placement {
            year,
            month,
            subdir: "photos",
            source: "live-photo",
        });
    }

    let (year, month, source) = month_dir_for(path)?;
    Ok(Placement {
        year,
        month,
        subdir: media_subdir(path),
        source,
    })
}

/// Return (year, month, source-of-date) for an image or video.
fn month_dir_for(path: &Path) -> Result<(i32, u32, &'static str), String> {
    if is_video(path) {
        if let Some((year, month)) = video_year_month(path) {
            return Ok((year, month, "video"));
        }
    } else if let Some((year, month)) = exif_year_month(path) {
        return Ok((year, month, "exif"));
    }
    let (year, month) = mtime_year_month(path)?;
    Ok((year, month, "mtime"))
}

/// Try to read the capture date from EXIF metadata.
fn exif_year_month(path: &Path) -> Option<(i32, u32)> {
    let file = fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let exif = exif::Reader::new().read_from_container(&mut reader).ok()?;

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

/// Try to read the capture date from a QuickTime/ISO-BMFF video's
/// `moov/mvhd` atom (covers mov, mp4, m4v, 3gp). Other containers return
/// `None` and fall back to mtime.
fn video_year_month(path: &Path) -> Option<(i32, u32)> {
    let file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let mut reader = BufReader::new(file);
    mvhd_year_month(&mut reader, len)
}

/// Locate `moov/mvhd` within an ISO-BMFF stream and decode its creation time.
fn mvhd_year_month<R: Read + Seek>(reader: &mut R, len: u64) -> Option<(i32, u32)> {
    let (moov_start, moov_end) = find_box(reader, 0, len, b"moov")?;
    let (mvhd_start, _) = find_box(reader, moov_start, moov_end, b"mvhd")?;

    reader.seek(SeekFrom::Start(mvhd_start)).ok()?;
    let mut version_flags = [0u8; 4];
    reader.read_exact(&mut version_flags).ok()?;

    let creation_secs = if version_flags[0] == 1 {
        let mut buf = [0u8; 8];
        reader.read_exact(&mut buf).ok()?;
        u64::from_be_bytes(buf)
    } else {
        let mut buf = [0u8; 4];
        reader.read_exact(&mut buf).ok()?;
        u32::from_be_bytes(buf) as u64
    };

    mac_time_to_year_month(creation_secs)
}

/// Scan the boxes in the half-open file region `[start, end)` for one whose
/// type equals `target`, returning the `[payload_start, box_end)` range of its
/// contents (i.e. just past the box header).
///
/// Box layout: 4-byte big-endian size, 4-byte type. A size of 1 means a 64-bit
/// size follows the type; a size of 0 means the box runs to `end`.
fn find_box<R: Read + Seek>(
    reader: &mut R,
    start: u64,
    end: u64,
    target: &[u8; 4],
) -> Option<(u64, u64)> {
    let mut pos = start;
    while pos + 8 <= end {
        reader.seek(SeekFrom::Start(pos)).ok()?;
        let mut header = [0u8; 8];
        reader.read_exact(&mut header).ok()?;

        let size32 = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
        let box_type = [header[4], header[5], header[6], header[7]];

        let (header_len, box_size) = if size32 == 1 {
            let mut ext = [0u8; 8];
            reader.read_exact(&mut ext).ok()?;
            (16u64, u64::from_be_bytes(ext))
        } else {
            (8u64, size32 as u64)
        };

        let box_end = if box_size == 0 {
            end
        } else if box_size < header_len {
            return None; // malformed: size smaller than its own header
        } else {
            pos.checked_add(box_size)?
        };

        if box_end <= pos || box_end > end {
            return None; // malformed or runs past the region
        }

        if &box_type == target {
            return Some((pos + header_len, box_end));
        }
        pos = box_end;
    }
    None
}

/// Convert a QuickTime/Mac-epoch timestamp (seconds since 1904-01-01 UTC) into
/// a local-time `(year, month)`. A zero timestamp means "unknown" and yields
/// `None` so the caller can fall back to mtime.
fn mac_time_to_year_month(mac_secs: u64) -> Option<(i32, u32)> {
    if mac_secs == 0 {
        return None;
    }
    let unix_secs = mac_secs as i64 - MAC_EPOCH_OFFSET_SECS;
    let utc = Utc.timestamp_opt(unix_secs, 0).single()?;
    let local = utc.with_timezone(&Local);
    Some((local.year(), local.month()))
}

/// Pick a destination path inside `dest_dir`, appending " (n)" on collision.
pub fn unique_dest(dest_dir: &Path, src: &Path) -> PathBuf {
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
pub fn move_file(src: &Path, dest: &Path) -> std::io::Result<()> {
    match fs::rename(src, dest) {
        Ok(()) => Ok(()),
        Err(_) => {
            fs::copy(src, dest)?;
            fs::remove_file(src)?;
            Ok(())
        }
    }
}

pub fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn detects_video_extensions_case_insensitively() {
        assert!(is_video(Path::new("clip.mp4")));
        assert!(is_video(Path::new("clip.MOV")));
        assert!(is_video(Path::new("a/b/movie.M4V")));
        assert!(!is_video(Path::new("photo.jpg")));
        assert!(!is_video(Path::new("notes.txt")));
    }

    #[test]
    fn images_and_videos_are_both_media() {
        assert!(is_media(Path::new("photo.JPG")));
        assert!(is_media(Path::new("clip.mov")));
        assert!(!is_media(Path::new("readme.md")));
    }

    #[test]
    fn media_subdir_splits_photos_and_videos() {
        assert_eq!(media_subdir(Path::new("clip.mp4")), "videos");
        assert_eq!(media_subdir(Path::new("clip.MOV")), "videos");
        assert_eq!(media_subdir(Path::new("photo.jpg")), "photos");
        assert_eq!(media_subdir(Path::new("raw.dng")), "photos");
    }

    #[test]
    fn live_photo_motion_matches_its_still() {
        let files = vec![
            PathBuf::from("/p/IMG_1234.HEIC"),
            PathBuf::from("/p/IMG_1234.MOV"),
            PathBuf::from("/p/standalone.mp4"),
        ];
        let index = index_images_by_stem(&files);

        // The .mov is matched to its .heic still (stem match, case-insensitive).
        assert_eq!(
            live_photo_still(Path::new("/p/IMG_1234.MOV"), &index),
            Some(&PathBuf::from("/p/IMG_1234.HEIC")),
        );
        // A lone video has no still.
        assert_eq!(
            live_photo_still(Path::new("/p/standalone.mp4"), &index),
            None
        );
        // A still is never itself a motion component.
        assert_eq!(
            live_photo_still(Path::new("/p/IMG_1234.HEIC"), &index),
            None
        );
    }

    #[test]
    fn same_stem_in_different_dir_is_not_a_pair() {
        let files = vec![PathBuf::from("/a/IMG_1.heic")];
        let index = index_images_by_stem(&files);
        assert_eq!(live_photo_still(Path::new("/b/IMG_1.mov"), &index), None);
    }

    fn unique_tmp_dir(tag: &str) -> PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("sort_images-{tag}-{nanos}"))
    }

    #[test]
    fn purges_ds_store_recursively() {
        let base = unique_tmp_dir("purge");
        let sub = base.join("2019 Fall").join("photos");
        fs::create_dir_all(&sub).unwrap();
        fs::write(base.join(".DS_Store"), b"x").unwrap();
        fs::write(sub.join(".DS_Store"), b"x").unwrap();
        fs::write(sub.join("keep.jpg"), b"x").unwrap();

        let removed = purge_ds_store(&base, false).unwrap();

        assert_eq!(removed, 2);
        assert!(!base.join(".DS_Store").exists());
        assert!(!sub.join(".DS_Store").exists());
        assert!(sub.join("keep.jpg").exists(), "other files are untouched");

        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn dry_run_counts_but_keeps_ds_store() {
        let base = unique_tmp_dir("purge-dry");
        fs::create_dir_all(&base).unwrap();
        fs::write(base.join(".DS_Store"), b"x").unwrap();

        let removed = purge_ds_store(&base, true).unwrap();

        assert_eq!(removed, 1);
        assert!(base.join(".DS_Store").exists(), "dry run deletes nothing");

        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn month_labels_are_zero_padded_two_digits() {
        assert!(is_month_label("01"));
        assert!(is_month_label("12"));
        assert!(!is_month_label("1")); // not zero-padded
        assert!(!is_month_label("00")); // out of range
        assert!(!is_month_label("13")); // out of range
        assert!(!is_month_label("Fall")); // old season name
    }

    #[test]
    fn month_buckets_are_recognised() {
        assert!(looks_like_bucket(Path::new("/p/2019 10")));
        assert!(looks_like_bucket(Path::new("/p/2026 01")));
        assert!(!looks_like_bucket(Path::new("/p/2019 Fall")));
        assert!(!looks_like_bucket(Path::new("/p/random")));
    }

    #[test]
    fn mac_epoch_zero_is_unknown() {
        assert_eq!(mac_time_to_year_month(0), None);
    }

    #[test]
    fn mac_epoch_maps_to_expected_year_month() {
        // 1970-01-01T00:00:00Z expressed in the Mac epoch.
        let mac_unix_epoch = MAC_EPOCH_OFFSET_SECS as u64;
        // 30 days past the Unix epoch is still January 1970 in UTC; assert the
        // year is correct (month may shift by timezone, so only check year).
        assert_eq!(
            mac_time_to_year_month(mac_unix_epoch).map(|(y, _)| y),
            Some(1970)
        );
    }

    /// Build a minimal ISO-BMFF stream: an `ftyp` box followed by a `moov`
    /// box containing an `mvhd` box with a version-0 creation time.
    fn synthetic_mp4(creation_mac_secs: u32) -> Vec<u8> {
        fn box_bytes(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
            let size = (8 + payload.len()) as u32;
            let mut out = size.to_be_bytes().to_vec();
            out.extend_from_slice(kind);
            out.extend_from_slice(payload);
            out
        }

        // mvhd payload: version (1) + flags (3) + creation_time (4) ...
        let mut mvhd_payload = vec![0u8; 4];
        mvhd_payload.extend_from_slice(&creation_mac_secs.to_be_bytes());
        // pad out the rest of a version-0 mvhd; contents are unread.
        mvhd_payload.extend_from_slice(&[0u8; 12]);
        let mvhd = box_bytes(b"mvhd", &mvhd_payload);

        let moov = box_bytes(b"moov", &mvhd);
        let ftyp = box_bytes(b"ftyp", b"isom\0\0\0\0isom");

        let mut out = ftyp;
        out.extend_from_slice(&moov);
        out
    }

    #[test]
    fn reads_creation_date_from_mvhd() {
        // 2011-10-25 in the Mac epoch (seconds since 1904-01-01 UTC).
        let unix = Utc
            .with_ymd_and_hms(2011, 10, 25, 12, 0, 0)
            .single()
            .unwrap()
            .timestamp();
        let mac_secs = (unix + MAC_EPOCH_OFFSET_SECS) as u32;

        let data = synthetic_mp4(mac_secs);
        let len = data.len() as u64;
        let mut cursor = Cursor::new(data);

        let (year, month) = mvhd_year_month(&mut cursor, len).expect("should parse mvhd");
        assert_eq!(year, 2011);
        // Use noon UTC so local-time conversion keeps the same calendar day.
        assert_eq!(month, 10);
    }

    #[test]
    fn missing_moov_returns_none() {
        let data = b"\0\0\0\x10ftypisom\0\0\0\0".to_vec();
        let len = data.len() as u64;
        let mut cursor = Cursor::new(data);
        assert_eq!(mvhd_year_month(&mut cursor, len), None);
    }
}
