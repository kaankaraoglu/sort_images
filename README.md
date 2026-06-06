<!-- Allow this file not to have a first-line heading -->
<!-- markdownlint-disable-file MD041 no-emphasis-as-heading -->

<!-- inline html -->
<!-- markdownlint-disable-file MD033 -->

<div align="center">

# 📷 `sort_images`

**Sort images and videos into configurable date folders based on the month they were taken**

[![dependency status](https://deps.rs/repo/github/kaankaraoglu/sort_images/status.svg)](https://deps.rs/repo/github/kaankaraoglu/sort_images)
[![CI](https://github.com/kaankaraoglu/sort_images/actions/workflows/build-lint-format.yml/badge.svg)](https://github.com/kaankaraoglu/sort_images/actions/workflows/build-lint-format.yml)
</div>

## About

`sort_images` walks a folder of images and videos and moves each one into a
date folder based on the month it was taken. By default that folder is
`YYYY-MM` (e.g. `2026-01`, `2011-10`), split by type into `photos` and `videos`
subfolders — but both the [folder format](#configuration) and the split are
configurable:

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

# Nest by year then month, and don't split photos/videos
./target/release/sort_images ~/Pictures --format '{year}/{month}' --no-split
```

| Flag | Description |
|------|-------------|
| `--dry-run` | Preview what would happen without moving any files. |
| `--recursive` | Descend into subfolders (already-sorted date buckets are skipped). |
| `--format <TEMPLATE>` | Override the date folder template (see [Configuration](#configuration)). |
| `--split` / `--no-split` | Force the `photos`/`videos` split on or off. |

## Configuration

The folder layout is configurable through an optional `config.toml` in the
directory you run `sort_images` from, with CLI flags taking precedence.
**Precedence (low → high):** built-in defaults → `config.toml` → CLI flags. If
no config file is present and no flags are given, the default `{year}-{month}`
layout with the photos/videos split is used (unchanged from earlier versions).

```toml
# config.toml
format = "{year}-{month}"   # date folder template
split  = true               # split into photos/ and videos/ subfolders
```

Copy [`config.example.toml`](config.example.toml) to `config.toml` to get
started. Both keys are optional; unknown keys are rejected so typos can't be
silently ignored.

### Template tokens

| Token | Renders as | Example |
|-------|-----------|---------|
| `{year}` | 4-digit year | `2026` |
| `{month}` | zero-padded month `01`–`12` | `01` |

Any other text is literal, and `/` nests folders:

| Template | Result |
|----------|--------|
| `{year}-{month}` | `2026-01` |
| `{year}/{month}` | `2026/01` |
| `{year}` | `2026` |

A template must contain at least one `{year}` or `{month}` token. Unknown tokens
(e.g. `{day}`), path traversal (`..`, leading `/`), and unbalanced braces are
rejected before any file is moved.

## Notes

- Name collisions are handled by appending ` (1)`, ` (2)`, … so nothing is overwritten.
- Already-created buckets (matching the configured `--format`) are skipped when
  running with `--recursive`.
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
