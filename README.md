<!-- Allow this file not to have a first-line heading -->
<!-- markdownlint-disable-file MD041 no-emphasis-as-heading -->

<!-- inline html -->
<!-- markdownlint-disable-file MD033 -->

<div align="center">

# 📷 `sort_images`

**Sort images and videos into `YYYY Season` folders based on the season they were taken**

[![dependency status](https://deps.rs/repo/github/kaankaraoglu/sort_images/status.svg)](https://deps.rs/repo/github/kaankaraoglu/sort_images)
[![CI](https://github.com/kaankaraoglu/sort_images/actions/workflows/build-lint-format.yml/badge.svg)](https://github.com/kaankaraoglu/sort_images/actions/workflows/build-lint-format.yml)
</div>

## About

`sort_images` walks a folder of images and videos and moves each one into a
`YYYY Season` folder (e.g. `2026 Winter`, `2011 Fall`) based on the season it
was taken, split by type into `photos` and `videos` subfolders:

```text
2019 Fall/
├── photos/
└── videos/
2023 Summer/
├── photos/
└── videos/
```

Seasons follow calendar quarters (Northern hemisphere):
Winter = Jan–Mar, Spring = Apr–Jun, Summer = Jul–Sep, Fall = Oct–Dec.

- **Date source (images):** EXIF capture date (`DateTimeOriginal` →
  `DateTimeDigitized` → `DateTime`).
- **Date source (videos):** the QuickTime/ISO-BMFF `moov/mvhd` creation time
  (mov, mp4, m4v, 3gp).
- **Fallback:** the file's last-modified time when no embedded date exists.
- **Live Photos:** a video sitting next to an image with the same base name
  (e.g. Apple's `IMG_1234.HEIC` + `IMG_1234.MOV`) is treated as the still's
  motion component — it's filed under `photos/` in the **same** season as the
  still, so the pair stays together.
- **Operation:** files are **moved** into the season folders, which are created
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
```

## Notes

- Name collisions are handled by appending ` (1)`, ` (2)`, … so nothing is overwritten.
- Already-created buckets (e.g. `2019 Fall`) are skipped when running with `--recursive`.
- Supported image extensions include jpg/jpeg/png/gif/bmp/tiff/webp/heic plus
  common RAW formats (cr2, nef, arw, dng, …); supported video extensions include
  mov/mp4/m4v/3gp/avi/mkv/webm. Edit `IMAGE_EXTS` / `VIDEO_EXTS` in
  `src/main.rs` to adjust.
- Video capture dates are read from the `mvhd` atom for the QuickTime/ISO-BMFF
  family (mov, mp4, m4v, 3gp); other video containers fall back to mtime.
- Always try `--dry-run` first on important photos.
