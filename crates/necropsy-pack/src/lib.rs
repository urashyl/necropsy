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

pub mod curl;
pub mod gen;
pub mod learn;
pub mod scenario;
pub mod seal;
pub mod site;

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
    /// The library: puzzles that are not part of the daily rotation. Always
    /// available, in order, with no release date. Any kind of puzzle, live or not.
    Library,
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
    /// Learn modules (by id) this puzzle is a worked example for, once it has
    /// closed. See docs/learn.md.
    #[serde(default)]
    pub learn: Vec<String>,
    /// For a generated pack: the answer is meant to be found as it is in the
    /// files (a user name in a log, say), so `pack-tool check-gen` does not
    /// refuse files that contain it. See docs/generators.md.
    #[serde(default)]
    pub answer_in_files: bool,
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
    Some(days_from_civil(y, m, d))
}

/// Days since 1970-01-01 of a date (Howard Hinnant's algorithm).
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date of a day number: (year, month, day).
pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// A file under `files/`, addressed by its path relative to `files/`
/// using `/` separators.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// `generator.wasm`, for a generated pack: each player's files and answer
    /// come from it (see [`gen`]), and `files` holds only what every player shares.
    pub generator: Option<Vec<u8>>,
}

/// The browser solver's script in a pack directory: see docs/browser-solver.md.
pub const BROWSER_SOLVER: &str = "solve.browser.js";

/// Hardest difficulty that must be solvable with the in-browser workspace alone.
pub const BROWSER_MAX_DIFFICULTY: u8 = 3;

impl Pack {
    /// Whether `guess` is the answer. The guess is normalized first.
    pub fn check(&self, guess: &str) -> bool {
        // A live or generated pack has no fixed answer: it differs per player.
        !self.answer_hash.is_empty() && hash_answer(&self.meta.id, guess) == self.answer_hash
    }

    /// Whether this pack must come with a [`BROWSER_SOLVER`] script: a daily of
    /// difficulty 1 to 3 that is generated or `sealed`. Players of those must be
    /// able to solve it in the browser, and each player's answer differs, so the
    /// script is what shows that they can.
    pub fn needs_browser_solver(&self, sealed: bool) -> bool {
        self.meta.track == Track::Daily
            && self.meta.difficulty <= BROWSER_MAX_DIFFICULTY
            && (sealed || self.is_generated())
    }

    /// Whether each player's files and answer come from a generator.
    pub fn is_generated(&self) -> bool {
        self.generator.is_some()
    }

    pub fn is_live(&self) -> bool {
        self.live.is_some()
    }

    /// The sandbox image of a container-backed live pack.
    pub fn container(&self) -> Option<&str> {
        self.live
            .as_ref()
            .and_then(|l| l.container.as_ref())
            .map(|c| c.image.as_str())
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

pub(crate) fn valid_id(id: &str) -> bool {
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
    load_pack_files(dir, read_pack_files(dir)?, None)
}

/// Every file of a pack directory, by path, with the same rules as [`load_pack`]
/// (no symlinks, size limits, a generator's `target/` skipped). What `pack-tool
/// seal` puts in a sealed pack.
pub fn read_pack_files(dir: &Path) -> Result<Vec<(String, Vec<u8>)>, PackError> {
    let mut raw: Vec<(String, Vec<u8>)> = Vec::new();
    let mut total = 0u64;
    walk(dir, dir, 0, &mut raw, &mut total)?;
    Ok(raw)
}

/// A pack from its files in memory, such as an opened sealed pack. `dir` only
/// names it in errors. `assign_date` gives a daily without a date one: a
/// fallback sealed pack, released in another's place (see seal.rs).
pub fn load_pack_files(
    dir: &Path,
    mut raw: Vec<(String, Vec<u8>)>,
    assign_date: Option<&str>,
) -> Result<Pack, PackError> {
    raw.sort_by(|a, b| a.0.cmp(&b.0));

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

    let mut meta: Meta = toml::from_str(&text("pack.toml")?)
        .map_err(|e| invalid(&dir.join("pack.toml"), e.to_string()))?;
    if let Some(d) = assign_date {
        if meta.track != Track::Daily || meta.date.is_some() {
            return Err(invalid(
                &dir.join("pack.toml"),
                "a fallback is a daily with no date of its own",
            ));
        }
        meta.date = Some(d.to_owned());
    }
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
        (Track::Library, Some(_)) => {
            return Err(invalid(
                &dir.join("pack.toml"),
                "library packs must not have a date",
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

    for h in &meta.hints {
        check_hint(h).map_err(|e| invalid(&dir.join("pack.toml"), e))?;
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

    let generator = raw
        .iter()
        .find(|(p, _)| p == "generator.wasm")
        .map(|(_, d)| d.clone());
    let has_source = raw.iter().any(|(p, _)| p.starts_with("generator/"));
    match (&generator, has_source) {
        (Some(_), false) => {
            return Err(invalid(
                &dir.join("generator"),
                "generator.wasm needs its source in generator/, for review",
            ))
        }
        (None, true) => {
            return Err(invalid(
                &dir.join("generator.wasm"),
                "missing: build it with `pack-tool gen-build`",
            ))
        }
        _ => {}
    }
    if let Some(wasm) = &generator {
        if is_live {
            return Err(invalid(
                &dir.join("generator.wasm"),
                "a live pack has no generator: its flag is already made per instance",
            ));
        }
        gen::check_module(wasm).map_err(|e| invalid(&dir.join("generator.wasm"), e))?;
    } else if meta.answer_in_files {
        return Err(invalid(
            &dir.join("pack.toml"),
            "answer_in_files is only for generated packs",
        ));
    }

    // A live pack's flag is made per instance, and a generated pack's answer per
    // player, so neither has an answer file.
    let answer_hash = if is_live || generator.is_some() {
        if has_raw("answer") {
            return Err(invalid(
                &dir.join("answer"),
                "live and generated packs have no answer file: theirs differs per player",
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
    if files.is_empty() && !is_live && generator.is_none() {
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
        generator,
    })
}

/// A hint may end with `{decoder: <CyberChef recipe>}`, which the site shows as a
/// link that opens the workspace's decoder with that recipe: everything from the
/// marker to the hint's last `}`. Anywhere but the end, or empty, it is an error.
pub fn check_hint(hint: &str) -> Result<(), String> {
    let Some(at) = hint.find("{decoder:") else {
        return Ok(());
    };
    let marker = &hint[at..];
    let recipe = marker
        .strip_prefix("{decoder:")
        .and_then(|r| r.trim_end().strip_suffix('}'))
        .map(str::trim);
    match recipe {
        Some(r) if !r.is_empty() && r.bytes().all(|b| (0x20..0x7f).contains(&b)) => Ok(()),
        _ => Err(format!(
            "hint {hint:?}: {{decoder: RECIPE}} must end the hint, with a recipe of printable ASCII"
        )),
    }
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
    // Intro stages first, in order, then dailies by release date, then live
    // puzzles in order.
    packs.sort_by_key(|p| {
        (
            match p.meta.track {
                Track::Intro => 0,
                Track::Daily => 1,
                Track::Library => 2,
            },
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
            // A generator's own build output, from `cargo test` in its directory.
            if path
                .strip_prefix(root)
                .is_ok_and(|r| r == Path::new("generator/target"))
            {
                continue;
            }
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
    fn civil_dates_round_trip() {
        for d in [-1000, 0, 1, 19_000, 20_784, 60_000] {
            let (y, m, dd) = civil_from_days(d);
            assert_eq!(days_from_civil(y, m, dd), d);
        }
        assert_eq!(
            civil_from_days(parse_date("2026-11-27").unwrap()),
            (2026, 11, 27)
        );
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
    fn decoder_links_in_hints() {
        assert!(check_hint("Plain hint.").is_ok());
        assert!(check_hint("Decode it. {decoder: From_Base64('A-Za-z0-9+/=',true)}").is_ok());
        assert!(check_hint("Decode it. {decoder: From_Hex('Auto')}  ").is_ok());
        // Recipes hold option objects in braces.
        assert!(
            check_hint("XOR. {decoder: XOR({'option':'Hex','string':'37'},'Standard',false)}")
                .is_ok()
        );
        for bad in [
            "{decoder: } empty",
            "{decoder:}",
            "{decoder: X} then more text",
            "{decoder: unclosed",
            "{decoder: é}",
        ] {
            assert!(check_hint(bad).is_err(), "{bad}");
        }
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
            "library-date",
            &[(
                "x",
                "track = \"library\"\nnumber = 1\ndate = \"2026-01-01\""
            )]
        )
        .contains("must not have a date"));
        // A library pack is any kind of puzzle: it does not need the live mode.
        let root = packs_dir("library", &[("x", "track = \"library\"\nnumber = 1")]);
        let loaded = load_all(&root);
        let _ = fs::remove_dir_all(&root);
        assert_eq!(loaded.unwrap()[0].meta.track, Track::Library);
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

#[cfg(test)]
mod browser_solver_tests {
    use super::*;

    fn repo_pack(id: &str) -> Pack {
        load_pack(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../packs")
                .join(id),
        )
        .unwrap()
    }

    #[test]
    fn generated_and_sealed_dailies_up_to_difficulty_three_need_a_browser_solver() {
        // A generated daily, difficulty 2: always.
        assert!(repo_pack("lockfile").needs_browser_solver(false));
        // A daily with one answer, difficulty 2: only once sealed.
        let plain = repo_pack("iloveyou-dropped");
        assert!(!plain.needs_browser_solver(false));
        assert!(plain.needs_browser_solver(true));
        // Difficulty 4, and packs that are not dailies: never.
        assert!(!repo_pack("peanuts").needs_browser_solver(true));
        assert!(!repo_pack("intro-log").needs_browser_solver(true));
        assert!(!repo_pack("skimmer").needs_browser_solver(true));
    }
}
