<!-- Allow this file not to have a first-line heading -->
<!-- markdownlint-disable-file MD041 no-emphasis-as-heading -->

<!-- inline html -->
<!-- markdownlint-disable-file MD033 -->

<div align="center">

# 📷 `sort_images`

**Sort images and videos into `YYYY-MM` folders based on the month they were taken**

[![dependency status](https://deps.rs/repo/github/kaankaraoglu/sort_images/status.svg)](https://deps.rs/repo/github/kaankaraoglu/sort_images)
[![CI](https://github.com/kaankaraoglu/sort_images/actions/workflows/build-lint-format.yml/badge.svg)](https://github.com/kaankaraoglu/sort_images/actions/workflows/build-lint-format.yml)
</div>

## About

`sort_images` walks a folder of images and videos and moves each one into a
`YYYY-MM` folder (e.g. `2026-01`, `2011-10`) based on the month it was taken,
split by type into `photos` and `videos` subfolders:

```text
2019-10/
├── photos/
└── videos/
2023-07/
├── photos/
└── videos/
```

The month is zero-padded to two digits so the folders sort chronologically
within a year.

- **Date source (images):** EXIF capture date (`DateTimeOriginal` →
  `DateTimeDigitized` → `DateTime`).
- **Date source (videos):** the QuickTime/ISO-BMFF `moov/mvhd` creation time
  (mov, mp4, m4v, 3gp).
- **Fallback:** the file's last-modified time when no embedded date exists.
- **Live Photos:** a video sitting next to an image with the same base name
  (e.g. Apple's `IMG_1234.HEIC` + `IMG_1234.MOV`) is treated as the still's
  motion component — it's filed under `photos/` in the **same** month as the
  still, so the pair stays together.
- **Operation:** files are **moved** into the month folders, which are created
  inside the folder you point it at.

## Build

```bash
cargo build --release
```

## Usage

```bash
# Sort everything in ~/Pictures
./target/release/sort_images ~/Pictures

# Preview first without touching anything
./target/release/sort_images ~/Pictures --dry-run

# Also descend into subfolders
./target/release/sort_images ~/Pictures --recursive

# Sort and also upload to Google Photos
./target/release/sort_images ~/Pictures --upload
```

| Flag | Description |
|------|-------------|
| `--dry-run` | Preview what would happen without moving files or making network calls. |
| `--recursive` | Descend into subfolders (already-sorted `YYYY-MM` buckets are skipped). |
| `--upload` | Also upload sorted media to Google Photos, mirroring each `YYYY-MM` group as an album. |

## Google Photos upload

### One-time Google Cloud setup

1. Go to the [Google Cloud Console](https://console.cloud.google.com/) and create a new project (or select an existing one).
2. Enable the **Photos Library API** for that project.
3. Under **APIs & Services → Credentials**, create an **OAuth 2.0 Client ID** of type **Desktop app**.
4. Copy the generated **Client ID** and **Client Secret**.

### Config file

Create the config file at `~/.config/sort_images/config.toml`:

```toml
# ~/.config/sort_images/config.toml
[google]
client_id = "xxxx.apps.googleusercontent.com"
client_secret = "yyyy"
```

### Usage

```bash
# Sort and also upload, mirroring YYYY-MM folders as albums
./target/release/sort_images ~/Pictures --upload

# Preview without sorting, uploading, or any network calls
./target/release/sort_images ~/Pictures --upload --dry-run
```

### Behavior

- **First run** opens a browser for OAuth consent; tokens are cached at `~/.config/sort_images/token.json` for subsequent runs.
- An upload ledger at `~/.config/sort_images/ledger.json` makes `--upload` safely repeatable — already-uploaded files are skipped automatically.
- The `appendonly` scope means the tool only ever sees and manages photos it uploaded; it cannot read your existing library.
- Live Photo `.HEIC`/`.MOV` pairs upload as two media items into the same month album, keeping the pair together.

## Notes

- Name collisions are handled by appending ` (1)`, ` (2)`, … so nothing is overwritten.
- Already-created buckets (e.g. `2019-10`) are skipped when running with `--recursive`.
- Supported image extensions include jpg/jpeg/png/gif/bmp/tiff/webp/heic plus
  common RAW formats (cr2, nef, arw, dng, …); supported video extensions include
  mov/mp4/m4v/3gp/avi/mkv/webm. Edit `IMAGE_EXTS` / `VIDEO_EXTS` in
  `src/sort.rs` to adjust.
- Video capture dates are read from the `mvhd` atom for the QuickTime/ISO-BMFF
  family (mov, mp4, m4v, 3gp); other video containers fall back to mtime.
- After sorting, stray `.DS_Store` files are removed recursively from the target
  directory and the count is logged (`--dry-run` reports the count without
  deleting anything).
- Always try `--dry-run` first on important photos.
