# sort_photos

Moves image files into `YYYY/Qn` folders (e.g. `2026/Q1`, `2011/Q4`) based on the
quarter each photo was taken.

- **Date source:** EXIF capture date (`DateTimeOriginal` → `DateTimeDigitized` →
  `DateTime`), falling back to the file's last-modified time when no EXIF date exists.
- **Operation:** files are **moved** into the quarter folders, which are created
  inside the folder you point it at.

## Build

```bash
cargo build --release
```

## Run

```bash
# Sort everything in ~/Pictures
./target/release/sort_photos ~/Pictures

# Preview first without touching anything
./target/release/sort_photos ~/Pictures --dry-run

# Also descend into subfolders
./target/release/sort_photos ~/Pictures --recursive
```

## Notes

- Name collisions are handled by appending ` (1)`, ` (2)`, … so nothing is overwritten.
- Already-created `Qn` buckets are skipped when running with `--recursive`.
- Supported extensions include jpg/jpeg/png/gif/bmp/tiff/webp/heic plus common RAW
  formats (cr2, nef, arw, dng, …). Edit `IMAGE_EXTS` in `src/main.rs` to adjust.
- Always try `--dry-run` first on important photos.
