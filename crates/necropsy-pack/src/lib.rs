// SPDX-License-Identifier: AGPL-3.0-or-later
//! The necropsy puzzle pack format, shared by every front end.
//!
//! A pack is a plain directory (see `docs/pack-format.md`):
//!
//! ```text
//! <pack>/
//!   pack.toml     metadata: id, number, title, difficulty, modes, tags, hints
//!   brief.md      the question
//!   files/        the excerpt (web) or file tree (CLI)
//!   answer        `sha256:<hex>` of the normalized answer
//!   explainer.md  the real finding, shown after solving
//!   live.toml     live packs only: the scenario served to each player (see `scenario`)
//!   live/         live packs only: files the scenario can send
//! ```
//!
//! This crate only reads packs. It never executes anything from them, and the
//! loader refuses anything that could escape the pack directory (symlinks,
//! `..` components, absolute paths) or that is unreasonably large, because a
//! pack may one day come from somewhere less trusted than this repository.

use std::fs;
use std::path::{Component, Path, PathBuf};

pub mod scenario;

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Largest single file the loader accepts.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// Largest total size of a pack.
pub const MAX_PACK_BYTES: u64 = 8 * 1024 * 1024;
/// Most files a pack may contain (metadata files included).
pub const MAX_FILES: usize = 256;
/// Deepest directory nesting allowed under `files/`.
const MAX_DEPTH: usize = 8;

#[derive(Debug, thiserror::Error)]
pub enum PackError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: {message}")]
    Invalid { path: PathBuf, message: String },
    #[error("{0}")]
    Collection(String),
}

fn invalid(path: &Path, message: impl Into<String>) -> PackError {
    PackError::Invalid {
        path: path.to_owned(),
        message: message.into(),
    }
}

fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> PackError + '_ {
    move |source| PackError::Io {
        path: path.to_owned(),
        source,
    }
}

/// Which front ends a pack is meant for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Web,
    Cli,
    /// A server the player talks to, described by `live.toml`.
    Live,
}

/// Where a pack appears. Intro stages are always available, in order. Daily
/// packs are released on their `date`: the latest released one is "today's"
/// puzzle, earlier ones stay playable, and later ones are hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Track {
    Intro,
    Daily,
}

/// Contents of `pack.toml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Meta {
    /// URL-safe slug, unique across all packs: `[a-z0-9-]`, 1 to 40 chars.
    pub id: String,
    pub track: Track,
    /// Release date (`YYYY-MM-DD`, UTC) of a daily pack. Required for daily
    /// packs, not allowed for intro stages, and unique among daily packs.
    #[serde(default)]
    pub date: Option<String>,
    /// Display number and order within the track. Unique within the track.
    pub number: u32,
    pub title: String,
    /// 1 (easy) to 5 (hard).
    pub difficulty: u8,
    #[serde(default = "default_modes")]
    pub modes: Vec<Mode>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Revealed one at a time, in order.
    #[serde(default)]
    pub hints: Vec<String>,
}

fn default_modes() -> Vec<Mode> {
    vec![Mode::Web]
}

impl Meta {
    /// A daily pack's release date as days since 1970-01-01, if it has one.
    pub fn release_day(&self) -> Option<i64> {
        self.date.as_deref().and_then(parse_date)
    }
}

/// `YYYY-MM-DD` as days since 1970-01-01, or `None` if it is not a real date.
pub fn parse_date(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let y: i64 = s[0..4].parse().ok()?;
    let m: i64 = s[5..7].parse().ok()?;
    let d: i64 = s[8..10].parse().ok()?;
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days_in_month = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => 28 + i64::from(leap),
        _ => return None,
    };
    if !(1..=days_in_month).contains(&d) {
        return None;
    }
    // Days from civil date (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

/// A file under `files/`, addressed by its path relative to `files/`
/// using `/` separators.
#[derive(Debug, Clone)]
pub struct PackFile {
    pub path: String,
    pub data: Vec<u8>,
}

impl PackFile {
    /// The contents as text, if they are valid UTF-8.
    pub fn text(&self) -> Option<&str> {
        std::str::from_utf8(&self.data).ok()
    }
}

#[derive(Debug, Clone)]
pub struct Pack {
    pub meta: Meta,
    pub brief_md: String,
    pub explainer_md: String,
    /// `sha256:<hex>`, as stored in the `answer` file.
    pub answer_hash: String,
    pub files: Vec<PackFile>,
    /// The scenario of a live pack, from `live.toml`.
    pub live: Option<scenario::Scenario>,
    /// Files under `live/`, which scenario responses may send.
    pub live_files: Vec<PackFile>,
    /// SHA-256 over every file in the pack, see [`hash_files`].
    pub pack_hash: String,
}

impl Pack {
    /// Whether `guess` is the answer. The guess is normalized first.
    pub fn check(&self, guess: &str) -> bool {
        // A live pack has no fixed answer: its flag differs per instance.
        !self.answer_hash.is_empty() && hash_answer(&self.meta.id, guess) == self.answer_hash
    }

    pub fn is_live(&self) -> bool {
        self.live.is_some()
    }

    pub fn live_file(&self, path: &str) -> Option<&PackFile> {
        self.live_files.iter().find(|f| f.path == path)
    }

    pub fn file(&self, path: &str) -> Option<&PackFile> {
        self.files.iter().find(|f| f.path == path)
    }
}

/// Normalizes an answer before hashing: trims, strips one layer of
/// surrounding quotes or backticks, lowercases, and collapses runs of
/// whitespace to a single space.
pub fn normalize_answer(raw: &str) -> String {
    let trimmed = raw
        .trim()
        .trim_matches(|c| matches!(c, '"' | '\'' | '`'))
        .trim();
    trimmed
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// The value stored in a pack's `answer` file.
///
/// The pack id is part of the hashed input so identical answers in different
/// packs do not share a hash. This is an answer check, not a secret: the hash
/// sits in a public repository, so a short or guessable answer can be
/// brute-forced from it. Pick answers with some entropy.
pub fn hash_answer(pack_id: &str, answer: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"necropsy:v1:");
    h.update(pack_id.as_bytes());
    h.update(b":");
    h.update(normalize_answer(answer).as_bytes());
    format!("sha256:{}", hex(&h.finalize()))
}

/// Hash of a set of `(relative path, contents)` pairs, order-independent.
/// Each entry is hashed as `path NUL length(u64 LE) contents`, so moving
/// bytes between files changes the result.
pub fn hash_files(files: &[(String, Vec<u8>)]) -> String {
    let mut sorted: Vec<&(String, Vec<u8>)> = files.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut h = Sha256::new();
    for (path, data) in sorted {
        h.update(path.as_bytes());
        h.update([0]);
        h.update((data.len() as u64).to_le_bytes());
        h.update(data);
    }
    format!("sha256:{}", hex(&h.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 40
        && !id.starts_with('-')
        && !id.ends_with('-')
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Loads one pack directory.
pub fn load_pack(dir: &Path) -> Result<Pack, PackError> {
    let mut raw: Vec<(String, Vec<u8>)> = Vec::new();
    let mut total = 0u64;
    walk(dir, dir, 0, &mut raw, &mut total)?;

    let take = |name: &str| -> Result<&Vec<u8>, PackError> {
        raw.iter()
            .find(|(p, _)| p == name)
            .map(|(_, d)| d)
            .ok_or_else(|| invalid(&dir.join(name), "missing"))
    };
    let text = |name: &str| -> Result<String, PackError> {
        String::from_utf8(take(name)?.clone())
            .map_err(|_| invalid(&dir.join(name), "not valid UTF-8"))
    };

    let meta: Meta = toml::from_str(&text("pack.toml")?)
        .map_err(|e| invalid(&dir.join("pack.toml"), e.to_string()))?;
    if !valid_id(&meta.id) {
        return Err(invalid(
            &dir.join("pack.toml"),
            "id must be 1-40 chars of a-z, 0-9 and '-', not starting or ending with '-'",
        ));
    }
    if !(1..=5).contains(&meta.difficulty) {
        return Err(invalid(&dir.join("pack.toml"), "difficulty must be 1-5"));
    }
    if meta.modes.is_empty() {
        return Err(invalid(&dir.join("pack.toml"), "modes must not be empty"));
    }
    match (meta.track, &meta.date) {
        (Track::Intro, Some(_)) => {
            return Err(invalid(
                &dir.join("pack.toml"),
                "intro packs must not have a date",
            ))
        }
        (Track::Daily, None) => {
            return Err(invalid(
                &dir.join("pack.toml"),
                "daily packs need a date (YYYY-MM-DD)",
            ))
        }
        (Track::Daily, Some(d)) if parse_date(d).is_none() => {
            return Err(invalid(
                &dir.join("pack.toml"),
                format!("{d} is not a valid YYYY-MM-DD date"),
            ))
        }
        _ => {}
    }

    let is_live = meta.modes.contains(&Mode::Live);
    let has_raw = |name: &str| raw.iter().any(|(p, _)| p == name);
    let live = if is_live {
        let has_file = |f: &str| raw.iter().any(|(p, _)| p.strip_prefix("live/") == Some(f));
        Some(
            scenario::Scenario::parse(&text("live.toml")?, has_file)
                .map_err(|e| invalid(&dir.join("live.toml"), e))?,
        )
    } else {
        if has_raw("live.toml") {
            return Err(invalid(
                &dir.join("live.toml"),
                "only packs with the live mode may have a live.toml",
            ));
        }
        None
    };

    // A live pack's flag is made per instance, so it has no answer file.
    let answer_hash = if is_live {
        if has_raw("answer") {
            return Err(invalid(
                &dir.join("answer"),
                "live packs have no answer file: their flag is generated per instance",
            ));
        }
        String::new()
    } else {
        let answer_hash = text("answer")?.trim().to_owned();
        let hex_part = answer_hash.strip_prefix("sha256:").unwrap_or("");
        if hex_part.len() != 64
            || !hex_part
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(invalid(
                &dir.join("answer"),
                "expected `sha256:` followed by 64 lowercase hex digits",
            ));
        }
        answer_hash
    };

    let files_under = |prefix: &str| -> Vec<PackFile> {
        raw.iter()
            .filter_map(|(p, d)| {
                p.strip_prefix(prefix).map(|rel| PackFile {
                    path: rel.to_owned(),
                    data: d.clone(),
                })
            })
            .collect()
    };
    let files = files_under("files/");
    if files.is_empty() && !is_live {
        return Err(invalid(&dir.join("files"), "needs at least one file"));
    }
    let live_files = files_under("live/");

    Ok(Pack {
        brief_md: text("brief.md")?,
        explainer_md: text("explainer.md")?,
        pack_hash: hash_files(&raw),
        meta,
        answer_hash,
        files,
        live,
        live_files,
    })
}

/// Loads every pack directory directly under `root`, ordered by number.
/// Fails if two packs share an id or a number.
pub fn load_all(root: &Path) -> Result<Vec<Pack>, PackError> {
    let mut dirs = Vec::new();
    for entry in fs::read_dir(root).map_err(io_err(root))? {
        let entry = entry.map_err(io_err(root))?;
        let ty = entry.file_type().map_err(io_err(root))?;
        if ty.is_dir() {
            dirs.push(entry.path());
        }
    }
    dirs.sort();

    let mut packs = dirs
        .iter()
        .map(|d| load_pack(d))
        .collect::<Result<Vec<_>, _>>()?;
    // Intro stages first, in order, then dailies by release date.
    packs.sort_by_key(|p| {
        (
            p.meta.track == Track::Daily,
            p.meta.release_day(),
            p.meta.number,
        )
    });

    for (i, a) in packs.iter().enumerate() {
        for b in &packs[i + 1..] {
            if a.meta.track == b.meta.track && a.meta.number == b.meta.number {
                return Err(PackError::Collection(format!(
                    "packs {} and {} share number {} in the {:?} track",
                    a.meta.id, b.meta.id, a.meta.number, a.meta.track
                )));
            }
            if a.meta.track == Track::Daily
                && b.meta.track == Track::Daily
                && a.meta.date == b.meta.date
            {
                return Err(PackError::Collection(format!(
                    "daily packs {} and {} share the date {}",
                    a.meta.id,
                    b.meta.id,
                    a.meta.date.as_deref().unwrap_or("?")
                )));
            }
        }
    }
    for (i, a) in packs.iter().enumerate() {
        if packs[i + 1..].iter().any(|b| b.meta.id == a.meta.id) {
            return Err(PackError::Collection(format!(
                "duplicate pack id {}",
                a.meta.id
            )));
        }
    }
    if packs.is_empty() {
        return Err(PackError::Collection(format!(
            "no packs found in {}",
            root.display()
        )));
    }
    Ok(packs)
}

/// Collects every regular file under `dir` as `(relative path, bytes)`.
/// Symlinks and special files are rejected rather than followed.
fn walk(
    root: &Path,
    dir: &Path,
    depth: usize,
    out: &mut Vec<(String, Vec<u8>)>,
    total: &mut u64,
) -> Result<(), PackError> {
    if depth > MAX_DEPTH {
        return Err(invalid(dir, "directory nesting too deep"));
    }
    let mut entries = fs::read_dir(dir)
        .map_err(io_err(dir))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(io_err(dir))?;
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        let ty = entry.file_type().map_err(io_err(&path))?;
        if ty.is_symlink() {
            return Err(invalid(&path, "symlinks are not allowed in packs"));
        }
        if ty.is_dir() {
            walk(root, &path, depth + 1, out, total)?;
        } else if ty.is_file() {
            let len = entry.metadata().map_err(io_err(&path))?.len();
            if len > MAX_FILE_BYTES {
                return Err(invalid(
                    &path,
                    format!("larger than {MAX_FILE_BYTES} bytes"),
                ));
            }
            *total += len;
            if *total > MAX_PACK_BYTES || out.len() >= MAX_FILES {
                return Err(invalid(root, "pack is too large"));
            }
            let rel = relative_path(root, &path)?;
            out.push((rel, fs::read(&path).map_err(io_err(&path))?));
        } else {
            return Err(invalid(
                &path,
                "only regular files and directories are allowed",
            ));
        }
    }
    Ok(())
}

/// `path` relative to `root`, `/`-separated, with every component checked.
fn relative_path(root: &Path, path: &Path) -> Result<String, PackError> {
    let rel = path
        .strip_prefix(root)
        .map_err(|_| invalid(path, "outside the pack directory"))?;
    let mut parts = Vec::new();
    for c in rel.components() {
        match c {
            Component::Normal(s) => {
                let s = s
                    .to_str()
                    .ok_or_else(|| invalid(path, "file name is not valid UTF-8"))?;
                if s.contains('\\') || s.chars().any(char::is_control) {
                    return Err(invalid(path, "unsafe character in file name"));
                }
                parts.push(s.to_owned());
            }
            _ => return Err(invalid(path, "unsafe path component")),
        }
    }
    Ok(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization() {
        assert_eq!(normalize_answer("  Foo   BAR \n"), "foo bar");
        assert_eq!(normalize_answer("`Host.Example`"), "host.example");
        assert_eq!(normalize_answer("\"x\""), "x");
        assert_eq!(normalize_answer("  "), "");
    }

    #[test]
    fn answer_hash_depends_on_pack_and_normalization() {
        let a = hash_answer("one", "Secret");
        assert_eq!(a, hash_answer("one", "  secret "));
        assert_ne!(a, hash_answer("two", "secret"));
        assert!(a.starts_with("sha256:") && a.len() == 71);
    }

    #[test]
    fn pack_hash_is_order_independent_and_content_sensitive() {
        let a = vec![
            ("a".to_owned(), b"1".to_vec()),
            ("b".to_owned(), b"2".to_vec()),
        ];
        let b = vec![
            ("b".to_owned(), b"2".to_vec()),
            ("a".to_owned(), b"1".to_vec()),
        ];
        assert_eq!(hash_files(&a), hash_files(&b));
        let moved = vec![
            ("a".to_owned(), b"12".to_vec()),
            ("b".to_owned(), Vec::new()),
        ];
        assert_ne!(hash_files(&a), hash_files(&moved));
    }

    #[test]
    fn dates() {
        assert_eq!(parse_date("1970-01-01"), Some(0));
        assert_eq!(parse_date("1970-01-02"), Some(1));
        assert_eq!(parse_date("2000-03-01"), Some(11017));
        assert_eq!(parse_date("2026-11-24"), Some(20781));
        assert!(parse_date("2024-02-29").is_some());
        for bad in [
            "2023-02-29",
            "2026-13-01",
            "2026-00-10",
            "2026-11-31",
            "26-11-24",
            "2026/11/24",
            "2026-1-24",
            "",
        ] {
            assert_eq!(parse_date(bad), None, "{bad}");
        }
        assert_eq!(
            parse_date("2026-11-25").unwrap() - parse_date("2026-11-24").unwrap(),
            1
        );
    }

    #[test]
    fn id_validation() {
        assert!(valid_id("phone-home"));
        assert!(!valid_id("Phone"));
        assert!(!valid_id("-x"));
        assert!(!valid_id("a/b"));
        assert!(!valid_id(""));
    }

    /// A throwaway directory of packs for loader tests.
    fn packs_dir(name: &str, packs: &[(&str, &str)]) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("necropsy-pack-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for (id, extra) in packs {
            let d = root.join(id);
            fs::create_dir_all(d.join("files")).unwrap();
            fs::write(
                d.join("pack.toml"),
                format!("id = \"{id}\"\ntitle = \"T\"\ndifficulty = 1\n{extra}\n"),
            )
            .unwrap();
            fs::write(d.join("brief.md"), "b").unwrap();
            fs::write(d.join("explainer.md"), "e").unwrap();
            fs::write(d.join("files/f.txt"), "x").unwrap();
            fs::write(d.join("answer"), hash_answer(id, "x")).unwrap();
        }
        root
    }

    fn load_error(name: &str, packs: &[(&str, &str)]) -> String {
        let root = packs_dir(name, packs);
        let err = load_all(&root)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        let _ = fs::remove_dir_all(&root);
        err
    }

    #[test]
    fn tracks_are_ordered_and_validated() {
        // Intro stages come first in number order, then dailies by date.
        let root = packs_dir(
            "order",
            &[
                (
                    "d-late",
                    "track = \"daily\"\nnumber = 2\ndate = \"2026-02-02\"",
                ),
                ("i-two", "track = \"intro\"\nnumber = 2"),
                (
                    "d-early",
                    "track = \"daily\"\nnumber = 1\ndate = \"2026-01-01\"",
                ),
                ("i-one", "track = \"intro\"\nnumber = 1"),
            ],
        );
        let ids: Vec<String> = load_all(&root)
            .unwrap()
            .into_iter()
            .map(|p| p.meta.id)
            .collect();
        let _ = fs::remove_dir_all(&root);
        assert_eq!(ids, ["i-one", "i-two", "d-early", "d-late"]);

        assert!(load_error(
            "a",
            &[("x", "track = \"intro\"\nnumber = 1\ndate = \"2026-01-01\"")]
        )
        .contains("must not have a date"));
        assert!(load_error("b", &[("x", "track = \"daily\"\nnumber = 1")]).contains("need a date"));
        assert!(load_error(
            "c",
            &[("x", "track = \"daily\"\nnumber = 1\ndate = \"2026-02-30\"")]
        )
        .contains("not a valid"));
        assert!(
            load_error("d", &[("x", "number = 1")]).contains("track"),
            "a track is required"
        );
        let dup_date = load_error(
            "e",
            &[
                ("x", "track = \"daily\"\nnumber = 1\ndate = \"2026-01-01\""),
                ("y", "track = \"daily\"\nnumber = 2\ndate = \"2026-01-01\""),
            ],
        );
        assert!(dup_date.contains("share the date"), "{dup_date}");
        let dup_number = load_error(
            "f",
            &[
                ("x", "track = \"intro\"\nnumber = 1"),
                ("y", "track = \"intro\"\nnumber = 1"),
            ],
        );
        assert!(dup_number.contains("share number"), "{dup_number}");
        // The same number in different tracks is fine.
        let ok = packs_dir(
            "g",
            &[
                ("x", "track = \"intro\"\nnumber = 1"),
                ("y", "track = \"daily\"\nnumber = 1\ndate = \"2026-01-01\""),
            ],
        );
        assert!(load_all(&ok).is_ok());
        let _ = fs::remove_dir_all(&ok);
    }
}
