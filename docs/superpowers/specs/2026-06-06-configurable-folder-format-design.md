# Configurable Folder Format — Design

Date: 2026-06-06

## Goal

Make the destination folder layout configurable instead of hardcoded
`YYYY-MM/{photos,videos}`. Two things become configurable:

1. **Date folder template** — a custom token string (e.g. `{year}-{month}`,
   `{year}/{month}`, `{year}`).
2. **photos/videos split** — a toggle for whether files are separated into
   `photos/` and `videos/` subfolders or kept flat in the date folder.

Configuration lives in a `config.toml` file, with CLI flags able to override
it.

## Configuration sources & precedence

Low → high:

1. Built-in defaults (`format = "{year}-{month}"`, `split = true`) — these match
   today's behavior, so existing users see no change.
2. `./config.toml` in the current working directory (optional).
3. CLI flags (`--format`, `--split` / `--no-split`).

### config.toml

Looked up at `./config.toml` (current working directory). Optional:

- **Absent:** use built-in defaults, no error.
- **Present but malformed or invalid:** fail fast with a clear error and a
  non-zero exit before any file is moved.
- **Unknown keys** (e.g. a typo `frmat`): rejected via serde
  `deny_unknown_fields`, so silent fallback to defaults can't hide a typo.

Schema (both keys optional):

```toml
format = "{year}-{month}"
split  = true
```

A `config.example.toml` is committed to the repo to document the schema.
`config.toml` is added to `.gitignore` so a user's local config placed in the
repo root is not accidentally committed.

### CLI flags

| Flag | Effect |
|------|--------|
| `--format <TEMPLATE>` | Override the date folder template. |
| `--split` / `--no-split` | Force the photos/videos split on or off (overrides config in either direction). |
| `--dry-run` | Unchanged. |
| `--recursive`, `-r` | Unchanged. |

`--split` and `--no-split` are mutually exclusive; supplying both is an error.

## Template tokens

| Token | Renders as | Example |
|-------|-----------|---------|
| `{year}` | 4-digit year | `2026` |
| `{month}` | zero-padded 2-digit month `01`–`12` | `01` |

Any other text is literal. A `/` separates path components (nesting). Examples:

- `{year}-{month}` → `2026-01`
- `{year}/{month}` → `2026/01`
- `{year}` → `2026`

Only year/month granularity is supported (matches what capture-date extraction
produces today; no day extraction added).

### Validation

Performed once when the template is parsed, before any file is processed:

- Must contain at least one recognized token (`{year}` or `{month}`). A template
  with no tokens would funnel every file into a single folder — rejected as a
  likely mistake.
- Unknown tokens (`{day}`, `{foo}`, …) → error listing the allowed tokens.
- Path-traversal / escape rejected: no `..` path component, no leading `/`
  (output must stay under the root).
- Unbalanced braces (`{year`) → error.

## Architecture

### New type: `FolderFormat` (in `src/sort.rs`)

Encapsulates a parsed, validated template.

- `FolderFormat::parse(template: &str) -> Result<FolderFormat, String>` —
  parses into a sequence of segments (`Year`, `Month`, `Literal(String)`,
  `Separator`) and runs validation.
- `render(&self, year: i32, month: u32) -> String` — produces the relative date
  path (e.g. `2026/01`).
- `is_bucket_root(&self, dir_name: &str) -> bool` — true if `dir_name` matches
  the **first** path segment of the template. Replaces today's hardcoded
  `looks_like_bucket` / `is_month_label`. This keeps the `--recursive`
  "skip already-sorted folders" behavior correct for whatever format is in use:
  - `{year}-{month}` → first segment matches `\d{4}-\d{2}`
  - `{year}/{month}` → first segment matches `\d{4}`
  - `{year}` → matches `\d{4}`

  The match is implemented by walking the first segment's parsed tokens against
  the directory name (digits for `{year}`/`{month}`, exact match for literals),
  not by constructing real regex.

### New type: `Settings`

The resolved, ready-to-use configuration:

```rust
pub struct Settings {
    pub format: FolderFormat,
    pub split: bool,
}
```

Resolution flow in `main.rs`:

1. `parse_args()` produces a `Config` holding the root path, `dry_run`,
   `recursive`, and *optional* overrides (`format: Option<String>`,
   `split: Option<bool>`).
2. Load `config.toml` if present into a `FileConfig` (serde,
   `deny_unknown_fields`, all fields `Option`).
3. Resolve: CLI override → file value → built-in default, for each of `format`
   and `split`. Parse the final template string into a `FolderFormat`.
4. Any error in steps 2–3 prints a clear message and exits non-zero before
   collecting media.

### Per-file logic — unchanged

`plan_destination` still returns
`Placement { year, month, subdir, source }`, where `subdir` is the *category*
(`photos` / `videos`, with Live Photo motion → `photos`). The split toggle and
template are configuration concerns kept out of the per-file planner.

`main.rs` composes the destination:

```text
date_path = settings.format.render(p.year, p.month)
dest_dir  = root / date_path            (split off)
dest_dir  = root / date_path / p.subdir (split on)
```

The dry-run/move/collision/`.DS_Store` flows are otherwise unchanged.

## Dependencies

Add to `Cargo.toml`:

- `serde` with the `derive` feature — typed config struct.
- `toml` — parse `config.toml`.

Idiomatic, strongly-typed parsing rather than a hand-rolled reader.

## Error handling

| Situation | Behavior |
|-----------|----------|
| `config.toml` absent | Use defaults, no error. |
| `config.toml` unreadable / not valid TOML | Error + non-zero exit. |
| Unknown key in `config.toml` | Error (deny_unknown_fields) + exit. |
| Invalid template (unknown token, no token, traversal, unbalanced) | Error listing the problem + exit. |
| Both `--split` and `--no-split` | Error + exit. |

All config/validation errors are reported before any file is moved.

## Testing

Unit tests (in `src/sort.rs`, alongside existing tests):

- `FolderFormat::parse` accepts `{year}-{month}`, `{year}/{month}`, `{year}`.
- `parse` rejects: unknown token `{day}`, no-token literal, `..` traversal,
  leading `/`, unbalanced braces.
- `render` output for several templates and `(year, month)` pairs, including
  zero-padding of single-digit months.
- `is_bucket_root` matches the right top-level folder for each template and
  rejects non-matching names (mirrors the existing `month_buckets_are_recognised`
  test).
- Config resolution precedence: CLI override beats file beats default, for both
  `format` and `split`.

## Documentation

Update `README.md`:

- Describe `config.toml` (location, schema, example) and the precedence rules.
- Document `--format`, `--split`/`--no-split`.
- Note that the default layout is unchanged (`{year}-{month}` with the split on).
- Mention `config.example.toml`.

## Out of scope

- Day-level granularity / `{day}` token.
- Renaming the `photos`/`videos` subfolders.
- Named presets or strftime-style patterns (custom token string only).
- Config discovery beyond `./config.toml` (no OS config dir, no `--config`
  path flag).
