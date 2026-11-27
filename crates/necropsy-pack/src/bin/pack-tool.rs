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

/// The `cargo build` for a generator. Variables a surrounding `cargo run` sets
/// are removed: `RUSTUP_TOOLCHAIN` above all, which would make the build use
/// the caller's toolchain instead of the one `packs/rust-toolchain.toml` pins.
fn cargo_command(gen_dir: &Path, target: &Path, rustflags: &str) -> Command {
    let mut c = Command::new("cargo");
    c.current_dir(gen_dir)
        .args([
            "build",
            "--release",
            "--locked",
            "--target",
            "wasm32-unknown-unknown",
        ])
        .env("CARGO_TARGET_DIR", target)
        .env("RUSTFLAGS", rustflags);
    for var in [
        "RUSTUP_TOOLCHAIN",
        "RUSTC",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "CARGO",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_BUILD_TARGET",
    ] {
        c.env_remove(var);
    }
    c
}

/// Builds one generator to WebAssembly and returns the module's bytes. The
/// toolchain is the one `packs/rust-toolchain.toml` pins (cargo runs in the
/// generator's directory), and paths are remapped, so the same source builds
/// the same bytes anywhere.
fn build(dir: &Path) -> Result<Vec<u8>, String> {
    let gen_dir = dir.join("generator");
    let root = std::env::current_dir().map_err(|e| e.to_string())?;
    let target = root.join("target/generators");
    let cargo_home = std::env::var("CARGO_HOME")
        .unwrap_or_else(|_| format!("{}/.cargo", std::env::var("HOME").unwrap_or_default()));
    let repo = root.to_string_lossy();
    let rustflags =
        format!("--remap-path-prefix={repo}=/necropsy --remap-path-prefix={cargo_home}=/cargo");
    let status = cargo_command(&gen_dir, &target, &rustflags)
        .status()
        .map_err(|e| format!("cannot run cargo: {e}"))?;
    if !status.success() {
        return Err(format!("building {} failed", gen_dir.display()));
    }
    let manifest =
        std::fs::read_to_string(gen_dir.join("Cargo.toml")).map_err(|e| e.to_string())?;
    let name = manifest
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
        let c = cargo_command(Path::new("g"), Path::new("t"), "f");
        let envs: Vec<(String, Option<String>)> = c
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        for var in ["RUSTUP_TOOLCHAIN", "RUSTC", "CARGO"] {
            assert!(
                envs.contains(&(var.to_owned(), None)),
                "{var} is not removed"
            );
        }
        assert_eq!(c.get_current_dir(), Some(Path::new("g")));
    }
}
