// SPDX-License-Identifier: AGPL-3.0-or-later
//! Authoring helper for pack writers.
//!
//! ```text
//! pack-tool hash <pack-id> <answer>              print the value for a pack's `answer` file
//! pack-tool validate <packs-dir> [learn]         load every pack, print id, number and pack hash;
//!                                                then check the Learn modules (default: the
//!                                                `learn` directory next to the packs)
//! pack-tool gen-build <packs-dir> [id...]        build generators to generator.wasm
//! pack-tool gen-build --check <packs-dir> [id...]  build, and fail if a generator.wasm differs
//! pack-tool gen <packs-dir> <id> <seed> [dir]    write one player's files, print their answer
//! pack-tool check-gen <packs-dir> [id...]        check generators over many seeds
//! ```
//!
//! See docs/generators.md.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use necropsy_pack::{gen, load_all, Pack};

/// The key `pack-tool gen` and `check-gen` derive seeds with. The site uses its
/// own secret, so these files are never a player's.
const TOOL_SECRET: &[u8] = b"necropsy pack-tool";
/// Seeds `check-gen` tries per pack.
const CHECK_SEEDS: usize = 24;

fn validate(dir: &str, learn: Option<&str>) -> ExitCode {
    let packs = match load_all(Path::new(dir)) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    for p in &packs {
        println!("{:>3}  {:<24} {}", p.meta.number, p.meta.id, p.pack_hash);
    }
    let learn_dir = match learn {
        Some(l) => Path::new(l).to_path_buf(),
        None => Path::new(dir)
            .parent()
            .unwrap_or(Path::new("."))
            .join("learn"),
    };
    match necropsy_pack::learn::load_all(&learn_dir, &packs) {
        Ok(modules) => {
            for m in &modules {
                println!(
                    "learn {:>2}  tier {}  {:<28} {} questions",
                    m.meta.number,
                    m.meta.tier,
                    m.meta.id,
                    m.meta.checkpoint.len()
                );
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Pack directories with a `generator/Cargo.toml`, limited to `ids` if any.
fn generator_dirs(packs: &Path, ids: &[&str]) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    let entries = std::fs::read_dir(packs).map_err(|e| format!("{}: {e}", packs.display()))?;
    for e in entries.flatten() {
        let d = e.path();
        let id = e.file_name().to_string_lossy().into_owned();
        if d.join("generator/Cargo.toml").is_file()
            && (ids.is_empty() || ids.contains(&id.as_str()))
        {
            out.push(d);
        }
    }
    out.sort();
    for id in ids {
        if !out.iter().any(|d| d.ends_with(id)) {
            return Err(format!("{id} has no generator/Cargo.toml"));
        }
    }
    Ok(out)
}

/// Generators are built through this path, a symlink to the checkout, so the
/// paths the compiler sees are the same on every machine. Cargo hashes a crate's
/// absolute path into its symbol names, and after LTO those hashes decide the
/// order of functions in the module: built from two different checkouts, the
/// same source gave the same bytes in another order.
const BUILD_ROOT: &str = "/tmp/necropsy-gen-src";

/// The toolchain `packs/rust-toolchain.toml` pins, such as `1.97.0`.
fn pinned_toolchain(packs: &Path) -> Result<String, String> {
    let path = packs.join("rust-toolchain.toml");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    text.lines()
        .find_map(|l| l.trim().strip_prefix("channel = \""))
        .and_then(|l| l.strip_suffix('"'))
        .map(str::to_owned)
        .ok_or_else(|| format!("{} has no channel", path.display()))
}

/// The `cargo build` for a generator: through [`BUILD_ROOT`], with the pinned
/// toolchain set explicitly, and without the variables a surrounding `cargo run`
/// sets (its own RUSTUP_TOOLCHAIN above all, which would replace the pinned one).
fn cargo_command(manifest: &Path, target: &Path, rustflags: &str, toolchain: &str) -> Command {
    let mut c = Command::new("cargo");
    for var in [
        "RUSTC",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "CARGO",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_BUILD_TARGET",
    ] {
        c.env_remove(var);
    }
    c.arg("build")
        .arg("--manifest-path")
        .arg(manifest)
        .args([
            "--release",
            "--locked",
            "--target",
            "wasm32-unknown-unknown",
        ])
        .env("RUSTUP_TOOLCHAIN", toolchain)
        .env("CARGO_TARGET_DIR", target)
        .env("RUSTFLAGS", rustflags);
    c
}

/// Points [`BUILD_ROOT`] at the checkout in `root`.
#[cfg(unix)]
fn link_build_root(root: &Path) -> Result<(), String> {
    let link = Path::new(BUILD_ROOT);
    if std::fs::read_link(link).ok().as_deref() == Some(root) {
        return Ok(());
    }
    if link.symlink_metadata().is_ok() {
        std::fs::remove_file(link).map_err(|e| format!("{BUILD_ROOT}: {e}"))?;
    }
    std::os::unix::fs::symlink(root, link).map_err(|e| format!("{BUILD_ROOT}: {e}"))
}

#[cfg(not(unix))]
fn link_build_root(_root: &Path) -> Result<(), String> {
    Err("generators are built on Linux or macOS (see docs/generators.md)".into())
}

/// Builds one generator to WebAssembly and returns the module's bytes. The same
/// source builds the same bytes anywhere: the toolchain is pinned, and every
/// path the compiler sees is under [`BUILD_ROOT`], remapped to `/necropsy`.
fn build(dir: &Path) -> Result<Vec<u8>, String> {
    let root = std::env::current_dir().map_err(|e| e.to_string())?;
    let id = dir
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let packs = dir.parent().unwrap_or(Path::new("packs"));
    let packs_rel = packs.strip_prefix(&root).unwrap_or(packs);
    let toolchain = pinned_toolchain(packs)?;
    link_build_root(&root)?;
    let base = Path::new(BUILD_ROOT);
    let manifest = base.join(packs_rel).join(&id).join("generator/Cargo.toml");
    let target = base.join("target/generators");
    let rustflags = format!("--remap-path-prefix={BUILD_ROOT}=/necropsy");
    let status = cargo_command(&manifest, &target, &rustflags, &toolchain)
        .status()
        .map_err(|e| format!("cannot run cargo: {e}"))?;
    if !status.success() {
        return Err(format!("building {} failed", manifest.display()));
    }
    let text =
        std::fs::read_to_string(dir.join("generator/Cargo.toml")).map_err(|e| e.to_string())?;
    let name = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("name = \""))
        .and_then(|l| l.strip_suffix('"'))
        .ok_or("generator/Cargo.toml has no name")?
        .replace('-', "_");
    let wasm = target.join(format!("wasm32-unknown-unknown/release/{name}.wasm"));
    std::fs::read(&wasm).map_err(|e| format!("{}: {e}", wasm.display()))
}

fn gen_build(packs: &str, ids: &[&str], check: bool) -> ExitCode {
    let dirs = match generator_dirs(Path::new(packs), ids) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut ok = true;
    for d in dirs {
        let id = d
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        match build(&d) {
            Ok(bytes) => {
                let path = d.join("generator.wasm");
                if check {
                    let committed = std::fs::read(&path).unwrap_or_default();
                    if committed == bytes {
                        println!("{id}: generator.wasm matches its source");
                    } else {
                        eprintln!(
                            "error: {id}: generator.wasm does not match a build of generator/ \
                             (run `pack-tool gen-build packs {id}` and commit the result)"
                        );
                        eprintln!("{}", describe_difference(&committed, &bytes));
                        ok = false;
                    }
                } else if let Err(e) = std::fs::write(&path, &bytes) {
                    eprintln!("error: {}: {e}", path.display());
                    ok = false;
                } else {
                    println!("{id}: wrote generator.wasm, {} bytes", bytes.len());
                }
            }
            Err(e) => {
                eprintln!("error: {id}: {e}");
                ok = false;
            }
        }
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Where two builds differ, to find what made a build not reproducible: the
/// sizes, the first differing offset with the bytes around it, and the readable
/// strings only one of them has (paths and names are the usual cause).
fn describe_difference(committed: &[u8], built: &[u8]) -> String {
    let first = committed
        .iter()
        .zip(built)
        .position(|(a, b)| a != b)
        .unwrap_or(committed.len().min(built.len()));
    let hex = |b: &[u8]| {
        let from = first.saturating_sub(16);
        let to = (first + 32).min(b.len());
        b.get(from..to)
            .unwrap_or_default()
            .iter()
            .map(|x| format!("{x:02x}"))
            .collect::<String>()
    };
    let strings = |b: &[u8]| -> std::collections::BTreeSet<String> {
        b.split(|c| !(0x20..0x7f).contains(c))
            .filter(|s| s.len() >= 8)
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect()
    };
    let (a, b) = (strings(committed), strings(built));
    let only = |x: &std::collections::BTreeSet<String>, y: &std::collections::BTreeSet<String>| {
        x.difference(y).take(20).cloned().collect::<Vec<_>>()
    };
    format!(
        "  committed {} bytes, built {} bytes, first difference at {first:#x}\n  committed: {}\n  built:     {}\n  strings only in the committed one: {:?}\n  strings only in the build: {:?}",
        committed.len(),
        built.len(),
        hex(committed),
        hex(built),
        only(&a, &b),
        only(&b, &a)
    )
}

fn find(packs: &str, id: &str) -> Result<Pack, String> {
    load_all(Path::new(packs))
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|p| p.meta.id == id)
        .ok_or_else(|| format!("no pack {id}"))
}

fn gen_one(packs: &str, id: &str, seed: &str, out: Option<&str>) -> ExitCode {
    let result = find(packs, id).and_then(|p| {
        let wasm = p
            .generator
            .as_deref()
            .ok_or(format!("{id} has no generator"))?;
        let o = gen::run(wasm, gen::seed_for(TOOL_SECRET, id, seed))?;
        let dir = PathBuf::from(
            out.map(str::to_owned)
                .unwrap_or(format!("target/gen/{id}/{seed}")),
        );
        for f in &o.files {
            let path = dir.join(&f.path);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::write(&path, &f.data).map_err(|e| e.to_string())?;
            eprintln!("{}  {} bytes", path.display(), f.data.len());
        }
        println!("{}", o.answer);
        Ok(())
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The checks a generator must pass, over [`CHECK_SEEDS`] seeds:
/// - it runs within its limits and is deterministic (two runs, same output);
/// - different seeds give different answers;
/// - no answer is in the pack's own text (brief, explainer, pack.toml, files,
///   templates; generator/src is exempt, since lists of candidate values live there);
/// - unless `answer_in_files`, no answer is in the generated files as it is.
pub fn check_pack(p: &Pack, plaintext: &[(String, Vec<u8>)]) -> Result<String, String> {
    let wasm = p.generator.as_deref().ok_or("no generator")?;
    let id = &p.meta.id;
    let mut answers = std::collections::HashSet::new();
    let started = std::time::Instant::now();
    for i in 0..CHECK_SEEDS {
        let seed = gen::seed_for(TOOL_SECRET, id, &format!("check-{i}"));
        let a = gen::run(wasm, seed)?;
        let b = gen::run(wasm, seed)?;
        if a != b {
            return Err(format!(
                "seed {i}: two runs gave different output: it is not deterministic"
            ));
        }
        let answer = necropsy_pack::normalize_answer(&a.answer);
        for (path, data) in plaintext {
            let text = String::from_utf8_lossy(data).to_lowercase();
            if text.contains(&answer) {
                return Err(format!("seed {i}: the answer {:?} is in {path}", a.answer));
            }
        }
        if !p.meta.answer_in_files {
            for f in &a.files {
                if String::from_utf8_lossy(&f.data)
                    .to_lowercase()
                    .contains(&answer)
                {
                    return Err(format!(
                        "seed {i}: the answer {:?} is in the generated {} as it is \
                         (set answer_in_files = true if it is meant to be found there)",
                        a.answer, f.path
                    ));
                }
            }
        }
        answers.insert(answer);
    }
    // Allow a rare collision; a generator with a handful of answers fails.
    if answers.len() + 1 < CHECK_SEEDS {
        return Err(format!(
            "{CHECK_SEEDS} seeds gave only {} different answers: an answer shared by friends would work for too many players",
            answers.len()
        ));
    }
    Ok(format!(
        "{CHECK_SEEDS} seeds, {} answers, {:.1} ms a run",
        answers.len(),
        started.elapsed().as_secs_f64() * 1000.0 / (2 * CHECK_SEEDS) as f64
    ))
}

/// The pack's own text that must not hold an answer.
fn plaintext_of(dir: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(root: &Path, d: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        let Ok(entries) = std::fs::read_dir(d) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            let rel = p
                .strip_prefix(root)
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/");
            if rel == "generator.wasm"
                || rel.starts_with("generator/src")
                || rel.starts_with("generator/target")
                || rel.starts_with("generator/Cargo.")
            {
                continue;
            }
            if p.is_dir() {
                walk(root, &p, out);
            } else if let Ok(data) = std::fs::read(&p) {
                out.push((rel, data));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out
}

fn check_gen(packs: &str, ids: &[&str]) -> ExitCode {
    let all = match load_all(Path::new(packs)) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut ok = true;
    let mut n = 0;
    for p in all.iter().filter(|p| p.is_generated()) {
        if !ids.is_empty() && !ids.contains(&p.meta.id.as_str()) {
            continue;
        }
        n += 1;
        let plain = plaintext_of(&Path::new(packs).join(&p.meta.id));
        match check_pack(p, &plain) {
            Ok(msg) => println!("{}: ok, {msg}", p.meta.id),
            Err(e) => {
                eprintln!("error: {}: {e}", p.meta.id);
                ok = false;
            }
        }
    }
    if n == 0 && !ids.is_empty() {
        eprintln!("error: none of {ids:?} is a generated pack");
        ok = false;
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn usage() -> ExitCode {
    eprintln!(
        "usage: pack-tool hash <pack-id> <answer>
       pack-tool validate <packs-dir> [learn-dir]
       pack-tool gen-build [--check] <packs-dir> [id...]
       pack-tool gen <packs-dir> <id> <seed> [out-dir]
       pack-tool check-gen <packs-dir> [id...]"
    );
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    match a.as_slice() {
        ["hash", id, answer] => {
            println!("{}", necropsy_pack::hash_answer(id, answer));
            ExitCode::SUCCESS
        }
        ["validate", dir] => validate(dir, None),
        ["validate", dir, learn] => validate(dir, Some(learn)),
        ["gen-build", "--check", dir, ids @ ..] => gen_build(dir, ids, true),
        ["gen-build", dir, ids @ ..] => gen_build(dir, ids, false),
        ["gen", dir, id, seed] => gen_one(dir, id, seed, None),
        ["gen", dir, id, seed, out] => gen_one(dir, id, seed, Some(out)),
        ["check-gen", dir, ids @ ..] => check_gen(dir, ids),
        _ => usage(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A regression: run under `cargo run`, the build inherited RUSTUP_TOOLCHAIN
    /// and used the caller's toolchain, not the pinned one.
    #[test]
    fn the_build_uses_the_pinned_toolchain_whoever_runs_it() {
        let c = cargo_command(Path::new("g/Cargo.toml"), Path::new("t"), "f", "1.97.0");
        let envs: Vec<(String, Option<String>)> = c
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        for var in ["RUSTC", "CARGO"] {
            assert!(
                envs.contains(&(var.to_owned(), None)),
                "{var} is not removed"
            );
        }
        // Set, not inherited: whoever runs pack-tool, the pinned toolchain builds.
        assert!(envs.contains(&("RUSTUP_TOOLCHAIN".to_owned(), Some("1.97.0".to_owned()))));
        let args: Vec<String> = c
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(args
            .windows(2)
            .any(|w| w == ["--manifest-path", "g/Cargo.toml"]));
    }

    #[test]
    fn the_pinned_toolchain_is_read_from_packs() {
        let packs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs");
        let t = pinned_toolchain(&packs).unwrap();
        assert!(t.chars().next().unwrap().is_ascii_digit(), "{t}");
    }

    #[test]
    fn a_difference_is_described() {
        let d = describe_difference(
            b"same start /a/path/one end",
            b"same start /b/path/one end!",
        );
        assert!(
            d.contains("committed 26 bytes, built 27 bytes, first difference at 0xc"),
            "{d}"
        );
        assert!(
            d.contains("/a/path/one end") && d.contains("/b/path/one end!"),
            "{d}"
        );
    }
}
