<!-- Allow this file not to have a first-line heading -->
<!-- markdownlint-disable-file MD041 no-emphasis-as-heading -->

<!-- inline html -->
<!-- markdownlint-disable-file MD033 -->

<div align="center">

# 📷 `sort_images`

**Sort images into `YYYY/Qn` folders based on the quarter they were taken**

[![dependency status](https://deps.rs/repo/github/kaankaraoglu/sort_images/status.svg)](https://deps.rs/repo/github/kaankaraoglu/sort_images)
[![CI](https://github.com/kaankaraoglu/sort_images/actions/workflows/build-lint-format.yml/badge.svg)](https://github.com/kaankaraoglu/sort_images/actions/workflows/build-lint-format.yml)
</div>

## About

`sort_images` walks a folder of images and moves each one into a `YYYY/Qn`
subfolder (e.g. `2026/Q1`, `2011/Q4`) based on the quarter it was taken.

- **Date source:** EXIF capture date (`DateTimeOriginal` → `DateTimeDigitized` →
  `DateTime`), falling back to the file's last-modified time when no EXIF date exists.
- **Operation:** files are **moved** into the quarter folders, which are created
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
- Already-created `Qn` buckets are skipped when running with `--recursive`.
- Supported extensions include jpg/jpeg/png/gif/bmp/tiff/webp/heic plus common RAW
  formats (cr2, nef, arw, dng, …). Edit `IMAGE_EXTS` in `src/main.rs` to adjust.
- Always try `--dry-run` first on important photos.
