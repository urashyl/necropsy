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
//! pack-tool solve-browser <packs-dir> [id...] [--seeds N]
//!                                                solve packs in a browser with their
//!                                                solve.browser.js (docs/browser-solver.md)
//!
//! pack-tool seal <id> [--site URL] [--fallback] [--src DIR] [--sealed DIR]
//!                                                seal packs-src/<id> into packs-sealed/
//! pack-tool open <id> [--site URL] [--sealed DIR] [--out DIR]
//!                                                open a sealed pack: with the master key if
//!                                                set, else with its released key from the site
//! pack-tool fetch-wrap-key [--site URL] [--sealed DIR]
//!                                                save the site's wrap key in wrap-keys/
//! pack-tool check-sealed <packs-dir> [sealed-dir]  what CI checks, no secrets needed
//! pack-tool new-master-key <file>                write a new random master key (mode 0600)
//! pack-tool wrap-key                             print the master key's public wrap keys
//! pack-tool rewrap [--sealed DIR]                rotation: reseal every pack to the newest key
//! ```
//!
//! The site: `--site`, else `NECROPSY_BASE_URL` (environment, then `.env`), else
//! https://necropsy.dev. The master key: `NECROPSY_MASTER_KEY_FILE` or
//! `NECROPSY_MASTER_KEY`, never an argument. See docs/generators.md and
//! docs/sealing.md.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use necropsy_pack::seal::{self, Manifest, MasterKeys};
use necropsy_pack::{gen, load_all, site, Pack, BROWSER_MAX_DIFFICULTY, BROWSER_SOLVER};

/// The key `pack-tool gen` and `check-gen` derive seeds with. The site uses its
/// own secret, so these files are never a player's.
const TOOL_SECRET: &[u8] = b"necropsy pack-tool";

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
    let missing: Vec<&str> = packs
        .iter()
        .filter(|p| p.needs_browser_solver(false))
        .filter(|p| {
            !Path::new(dir)
                .join(&p.meta.id)
                .join(BROWSER_SOLVER)
                .is_file()
        })
        .map(|p| p.meta.id.as_str())
        .collect();
    if !missing.is_empty() {
        eprintln!(
            "error: a generated daily of difficulty 1 to {BROWSER_MAX_DIFFICULTY} needs a {BROWSER_SOLVER} \
             (see docs/browser-solver.md): {}",
            missing.join(", ")
        );
        return ExitCode::FAILURE;
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

/// The key `check-gen` derives its seeds with.
fn check_pack(p: &Pack, plaintext: &[(String, Vec<u8>)]) -> Result<String, String> {
    gen::check(p, plaintext, TOOL_SECRET)
}

/// The pack's own text that must not hold an answer.
fn plaintext_of(dir: &Path) -> Vec<(String, Vec<u8>)> {
    gen::plaintext(&necropsy_pack::read_pack_files(dir).unwrap_or_default())
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

// ---------------------------------------------------------------- sealing

/// `--name value` options, and the arguments that are not options.
struct Args {
    positional: Vec<String>,
    options: Vec<(String, Option<String>)>,
}

impl Args {
    /// Splits `args`; `flags` are the options that take no value.
    fn parse(args: &[&str], flags: &[&str]) -> Result<Self, String> {
        let mut positional = Vec::new();
        let mut options = Vec::new();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            if let Some(name) = a.strip_prefix("--") {
                if flags.contains(&name) {
                    options.push((name.to_owned(), None));
                } else {
                    let v = it.next().ok_or_else(|| format!("--{name} needs a value"))?;
                    options.push((name.to_owned(), Some((*v).to_owned())));
                }
            } else {
                positional.push((*a).to_owned());
            }
        }
        Ok(Self {
            positional,
            options,
        })
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.options
            .iter()
            .find(|(n, _)| n == name)
            .and_then(|(_, v)| v.as_deref())
    }

    fn flag(&self, name: &str) -> bool {
        self.options.iter().any(|(n, _)| n == name)
    }

    fn dir(&self, name: &str, default: &str) -> PathBuf {
        PathBuf::from(self.get(name).unwrap_or(default))
    }
}

/// Runs the browser solver (tools/browser-solver/solve.mjs) on each pack in
/// `packs` named in `ids`, or on every pack there with a solve script.
fn solve_browser(packs: &Path, ids: &[String], seeds: u32) -> Result<(), String> {
    let repo = packs
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let runner = repo.join("tools/browser-solver/solve.mjs");
    if !runner.is_file() {
        return Err(format!(
            "{} not found: run from the repository",
            runner.display()
        ));
    }
    let site_bin = match std::env::var_os("NECROPSY_WEB_BIN") {
        Some(b) => PathBuf::from(b),
        None => std::env::current_exe()
            .map_err(|e| e.to_string())?
            .with_file_name(format!("necropsy-web{}", std::env::consts::EXE_SUFFIX)),
    };
    if !site_bin.is_file() {
        return Err(format!(
            "{} not found: build it first (`cargo build --bin necropsy-web`) or set NECROPSY_WEB_BIN",
            site_bin.display()
        ));
    }
    let toolkit = std::env::var_os("NECROPSY_TOOLKIT")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo.join("toolkit/dist"));
    if !toolkit.is_dir() {
        return Err(format!(
            "{} not found: run toolkit/fetch.sh, or set NECROPSY_TOOLKIT",
            toolkit.display()
        ));
    }
    let dirs: Vec<PathBuf> = if ids.is_empty() {
        let mut d: Vec<PathBuf> = std::fs::read_dir(packs)
            .map_err(|e| format!("{}: {e}", packs.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.join(BROWSER_SOLVER).is_file())
            .collect();
        d.sort();
        d
    } else {
        ids.iter().map(|id| packs.join(id)).collect()
    };
    if dirs.is_empty() {
        return Err(format!(
            "no pack in {} has a {BROWSER_SOLVER}",
            packs.display()
        ));
    }
    let mut failed = Vec::new();
    for dir in &dirs {
        let status = Command::new("node")
            .arg(&runner)
            .arg("--pack")
            .arg(dir)
            .arg("--site-bin")
            .arg(&site_bin)
            .arg("--toolkit")
            .arg(&toolkit)
            .args(["--seeds", &seeds.to_string()])
            .status()
            .map_err(|e| format!("cannot run node: {e}"))?;
        if !status.success() {
            failed.push(dir.display().to_string());
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!("not solved in the browser: {}", failed.join(", ")))
    }
}

fn solve_browser_cmd(args: &Args) -> Result<(), String> {
    let [packs, ids @ ..] = args.positional.as_slice() else {
        return Err("usage: pack-tool solve-browser <packs-dir> [id...] [--seeds N]".into());
    };
    let seeds = match args.get("seeds") {
        Some(n) => n.parse().map_err(|_| "--seeds is a number")?,
        None => 5,
    };
    solve_browser(Path::new(packs), ids, seeds)
}

fn the_site(args: &Args) -> String {
    let (site, from) = site::resolve(
        args.get("site"),
        std::env::var("NECROPSY_BASE_URL").ok(),
        Some(Path::new(".env")),
    );
    eprintln!("site: {site} (from {from})");
    site
}

fn write_manifest(dir: &Path, m: &Manifest) -> Result<(), String> {
    std::fs::write(dir.join("manifest.toml"), m.to_toml()).map_err(|e| e.to_string())
}

fn seal_cmd(args: &Args) -> Result<(), String> {
    let [id] = args.positional.as_slice() else {
        return Err("usage: pack-tool seal <id> [--site URL] [--fallback]".into());
    };
    let src = args.dir("src", "packs-src").join(id);
    let out = args.dir("sealed", "packs-sealed");
    let fallback = args.flag("fallback");
    let raw = necropsy_pack::read_pack_files(&src).map_err(|e| e.to_string())?;
    // A fallback has no date of its own; any date checks that the rest loads.
    let pack = necropsy_pack::load_pack_files(&src, raw.clone(), fallback.then_some("2000-01-01"))
        .map_err(|e| e.to_string())?;
    if pack.meta.id != *id {
        return Err(format!("{} says its id is {}", src.display(), pack.meta.id));
    }
    if pack.meta.track != necropsy_pack::Track::Daily {
        return Err("only dailies are sealed: intro and library packs are public".into());
    }
    if pack.is_generated() {
        let report = check_pack(&pack, &gen::plaintext(&raw))?;
        eprintln!("{id}: generator ok, {report}");
    }
    if pack.needs_browser_solver(true) {
        if !src.join(BROWSER_SOLVER).is_file() {
            return Err(format!(
                "a daily of difficulty 1 to {BROWSER_MAX_DIFFICULTY} needs {}/{BROWSER_SOLVER} \
                 (see docs/browser-solver.md)",
                src.display()
            ));
        }
        let parent = src.parent().unwrap_or(Path::new("."));
        solve_browser(parent, std::slice::from_ref(id), 5)?;
    }
    let site = the_site(args);
    let key = site::fetch_wrap_key(&site)?;
    let committed = seal::committed_wrap_keys(&out)?;
    match committed.get(&key.version) {
        Some(c) if *c != key => {
            return Err(format!(
                "{site} serves a v{} wrap key that is not the one in {}/wrap-keys: \
                 a pack sealed to it could not be opened by the site the repository deploys to. \
                 Use --site for the right site.",
                key.version,
                out.display()
            ))
        }
        Some(_) => {}
        None => {
            return Err(format!(
                "{site}'s wrap key v{} is not committed in {}/wrap-keys: an admin adds it with \
                 `pack-tool fetch-wrap-key` (see docs/sealing.md)",
                key.version,
                out.display()
            ))
        }
    }
    if committed.keys().next_back() != Some(&key.version) {
        return Err(format!(
            "v{} is not the newest committed wrap key",
            key.version
        ));
    }
    // The generator's local build output never goes in a sealed pack.
    let (bytes, _) = seal::seal(id, &raw, &key)?;
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    std::fs::write(out.join(format!("{id}.sealed")), &bytes).map_err(|e| e.to_string())?;
    let mut manifest = Manifest::load(&out)?;
    manifest.upsert(seal::Entry {
        id: id.clone(),
        number: pack.meta.number,
        difficulty: pack.meta.difficulty,
        date: if fallback {
            None
        } else {
            pack.meta.date.clone()
        },
        fallback,
        key_version: key.version,
        sha256: seal::sha256_hex(&bytes),
    });
    write_manifest(&out, &manifest)?;
    println!(
        "sealed {id} to v{} ({} bytes): commit {}/{id}.sealed and {}/manifest.toml, never {}",
        key.version,
        bytes.len(),
        out.display(),
        out.display(),
        src.display()
    );
    Ok(())
}

fn open_cmd(args: &Args) -> Result<(), String> {
    let [id] = args.positional.as_slice() else {
        return Err("usage: pack-tool open <id> [--site URL] [--out DIR]".into());
    };
    let sealed_dir = args.dir("sealed", "packs-sealed");
    let out = args.dir("out", "packs-src").join(id);
    let bytes =
        std::fs::read(sealed_dir.join(format!("{id}.sealed"))).map_err(|e| e.to_string())?;
    let s = seal::parse(&bytes)?;
    let k = match MasterKeys::from_env()? {
        Some(m) => seal::unwrap_key(&s, &m)?,
        None => site::fetch_released_key(&the_site(args), id)?,
    };
    let files = seal::open(&s, &k)?;
    if out.exists() {
        return Err(format!("{} exists: move it first", out.display()));
    }
    for (p, d) in &files {
        let path = out.join(p);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, d).map_err(|e| e.to_string())?;
    }
    println!("opened {id} into {} ({} files)", out.display(), files.len());
    Ok(())
}

fn fetch_wrap_key_cmd(args: &Args) -> Result<(), String> {
    let out = args.dir("sealed", "packs-sealed").join("wrap-keys");
    let key = site::fetch_wrap_key(&the_site(args))?;
    let path = out.join(format!("v{}.pub", key.version));
    if let Ok(old) = std::fs::read_to_string(&path) {
        if seal::WrapKey::parse_file(&old)? != key {
            return Err(format!(
                "{} holds another v{} key: a version's key never changes; rotate to a new version instead",
                path.display(),
                key.version
            ));
        }
        println!("{} already holds this key", path.display());
        return Ok(());
    }
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    std::fs::write(&path, key.to_file()).map_err(|e| e.to_string())?;
    println!("wrote {}: commit it", path.display());
    Ok(())
}

fn master_from_env() -> Result<MasterKeys, String> {
    MasterKeys::from_env()?.ok_or_else(|| {
        "set NECROPSY_MASTER_KEY_FILE (or NECROPSY_MASTER_KEY) to the master key".to_owned()
    })
}

fn rewrap_cmd(args: &Args) -> Result<(), String> {
    let dir = args.dir("sealed", "packs-sealed");
    let master = master_from_env()?;
    let to = master.current_wrap_key();
    let mut manifest = Manifest::load(&dir)?;
    let mut n = 0;
    for e in manifest.pack.iter_mut() {
        if e.key_version == to.version {
            continue;
        }
        let path = dir.join(format!("{}.sealed", e.id));
        let bytes = std::fs::read(&path).map_err(|err| format!("{}: {err}", path.display()))?;
        let re = seal::rewrap(&bytes, &master, &to)?;
        std::fs::write(&path, &re).map_err(|err| err.to_string())?;
        e.key_version = to.version;
        e.sha256 = seal::sha256_hex(&re);
        n += 1;
    }
    write_manifest(&dir, &manifest)?;
    let keys = dir.join("wrap-keys");
    std::fs::create_dir_all(&keys).map_err(|e| e.to_string())?;
    std::fs::write(keys.join(format!("v{}.pub", to.version)), to.to_file())
        .map_err(|e| e.to_string())?;
    println!(
        "resealed {n} packs to v{}; commit packs-sealed/",
        to.version
    );
    Ok(())
}

fn new_master_key(path: &str) -> Result<(), String> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(path)
        .map_err(|e| format!("{path}: {e} (it is never overwritten)"))?;
    use std::io::Write;
    f.write_all(format!("{}\n", MasterKeys::generate_text()).as_bytes())
        .map_err(|e| e.to_string())?;
    println!("wrote a new master key to {path}. Keep it out of the repository and of CI.");
    Ok(())
}

fn check_sealed(packs: &str, sealed: Option<&str>) -> Result<(), String> {
    let plain = load_all(Path::new(packs)).map_err(|e| e.to_string())?;
    let dir = match sealed {
        Some(d) => PathBuf::from(d),
        None => Path::new(packs)
            .parent()
            .unwrap_or(Path::new("."))
            .join("packs-sealed"),
    };
    for line in seal::check_dir(&dir, &plain)? {
        println!("{line}");
    }
    println!("{}: ok", dir.display());
    Ok(())
}

fn done(r: Result<(), String>) -> ExitCode {
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn usage() -> ExitCode {
    eprintln!(
        "usage: pack-tool hash <pack-id> <answer>
       pack-tool validate <packs-dir> [learn-dir]
       pack-tool gen-build [--check] <packs-dir> [id...]
       pack-tool gen <packs-dir> <id> <seed> [out-dir]
       pack-tool check-gen <packs-dir> [id...]
       pack-tool solve-browser <packs-dir> [id...] [--seeds N]
       pack-tool seal <id> [--site URL] [--fallback] [--src DIR] [--sealed DIR]
       pack-tool open <id> [--site URL] [--sealed DIR] [--out DIR]
       pack-tool fetch-wrap-key [--site URL] [--sealed DIR]
       pack-tool check-sealed <packs-dir> [sealed-dir]
       pack-tool new-master-key <file>
       pack-tool wrap-key
       pack-tool rewrap [--sealed DIR]"
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
        ["seal", rest @ ..] => done(Args::parse(rest, &["fallback"]).and_then(|a| seal_cmd(&a))),
        ["open", rest @ ..] => done(Args::parse(rest, &[]).and_then(|a| open_cmd(&a))),
        ["fetch-wrap-key", rest @ ..] => {
            done(Args::parse(rest, &[]).and_then(|a| fetch_wrap_key_cmd(&a)))
        }
        ["solve-browser", rest @ ..] => {
            done(Args::parse(rest, &[]).and_then(|a| solve_browser_cmd(&a)))
        }
        ["check-sealed", packs] => done(check_sealed(packs, None)),
        ["check-sealed", packs, sealed] => done(check_sealed(packs, Some(sealed))),
        ["new-master-key", path] => done(new_master_key(path)),
        ["wrap-key"] => done(master_from_env().map(|m| {
            for v in m.versions() {
                print!("{}", m.wrap_key(v).expect("exists").to_file());
            }
        })),
        ["rewrap", rest @ ..] => done(Args::parse(rest, &[]).and_then(|a| rewrap_cmd(&a))),
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

    /// A sealed daily of difficulty 1 to 3 is refused without its browser
    /// solver, before any site is asked for a key.
    #[test]
    fn seal_refuses_an_easy_daily_without_a_browser_solver() {
        let root =
            std::env::temp_dir().join(format!("necropsy-seal-solver-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("src/easy-one");
        std::fs::create_dir_all(dir.join("files")).unwrap();
        std::fs::write(
            dir.join("pack.toml"),
            "id = \"easy-one\"\ntrack = \"daily\"\ndate = \"2030-01-01\"\nnumber = 900\ntitle = \"Easy\"\ndifficulty = 2\nmodes = [\"web\"]\n",
        )
        .unwrap();
        std::fs::write(dir.join("brief.md"), "What is the word?").unwrap();
        std::fs::write(dir.join("explainer.md"), "It was in the file.").unwrap();
        std::fs::write(dir.join("files/note.txt"), "the word is plinth\n").unwrap();
        std::fs::write(
            dir.join("answer"),
            format!("{}\n", necropsy_pack::hash_answer("easy-one", "plinth")),
        )
        .unwrap();
        let src = root.join("src").display().to_string();
        let sealed = root.join("sealed").display().to_string();
        let args = Args::parse(
            &[
                "easy-one",
                "--src",
                &src,
                "--sealed",
                &sealed,
                "--site",
                "http://127.0.0.1:9",
            ],
            &[],
        )
        .unwrap();
        let e = seal_cmd(&args).unwrap_err();
        assert!(e.contains("solve.browser.js"), "{e}");
        assert!(!root.join("sealed").exists(), "nothing written");
        let _ = std::fs::remove_dir_all(&root);
    }
}
