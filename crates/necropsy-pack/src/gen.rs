// SPDX-License-Identifier: AGPL-3.0-or-later
//! Running pack generators. See `docs/generators.md`.
//!
//! A generator is a WebAssembly module (`generator.wasm` in the pack) that turns
//! a seed into a player's files and expected answer. It is run here with
//! [wasmi], an interpreter, and:
//!
//! - **no imports**: a module that imports anything is refused before it runs,
//!   so it has no clock, randomness, files, network or host calls. The seed is
//!   its only input, which makes it deterministic.
//! - **fuel**: a cap on instructions executed, so a loop cannot run forever.
//! - **memory**: a cap on its linear memory.
//!
//! Its output is checked like a pack's own files: plain relative paths, at most
//! 64 files, 1 MiB each, 8 MiB in all.

use hmac::{Hmac, Mac};
use sha2::Sha256;
use wasmi::{Config, Engine, Linker, Module, Store, StoreLimits, StoreLimitsBuilder};

use crate::PackFile;

/// Instructions a generator may execute, roughly: one to two seconds.
pub const MAX_FUEL: u64 = 500_000_000;
/// Linear memory a generator may use.
pub const MAX_MEMORY: usize = 64 << 20;
/// Size of a `generator.wasm`.
pub const MAX_MODULE: usize = 4 << 20;
pub const MAX_FILES: usize = 64;
pub const MAX_FILE: usize = 1 << 20;
pub const MAX_TOTAL: usize = 8 << 20;
pub const MAX_ANSWER: usize = 512;
/// The exported entry point.
pub const ENTRY: &str = "necropsy_generate";

/// What a generator made for one seed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub answer: String,
    pub files: Vec<PackFile>,
}

/// The generator's seed for one player: HMAC-SHA-256 keyed with the site's
/// secret, over the pack id and the player's own random seed. Without the
/// secret, a player's stored seed says nothing about their answer.
pub fn seed_for(secret: &[u8], pack_id: &str, player_seed: &str) -> [u64; 4] {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret).expect("HMAC takes any key length");
    mac.update(b"necropsy:gen:v1:");
    mac.update(pack_id.as_bytes());
    mac.update(b":");
    mac.update(player_seed.as_bytes());
    let d = mac.finalize().into_bytes();
    let w = |i: usize| u64::from_le_bytes(d[i * 8..i * 8 + 8].try_into().expect("8 bytes"));
    [w(0), w(1), w(2), w(3)]
}

fn engine() -> Engine {
    let mut config = Config::default();
    config.consume_fuel(true);
    Engine::new(&config)
}

/// Checks a module without running it: it parses, imports nothing, and exports
/// `memory` and `necropsy_generate(i64, i64, i64, i64) -> i64`.
pub fn check_module(wasm: &[u8]) -> Result<(), String> {
    if wasm.len() > MAX_MODULE {
        return Err(format!("generator.wasm is larger than {MAX_MODULE} bytes"));
    }
    let module =
        Module::new(&engine(), wasm).map_err(|e| format!("not a valid WebAssembly module: {e}"))?;
    if let Some(i) = module.imports().next() {
        return Err(format!(
            "the generator imports {}::{}; generators may import nothing",
            i.module(),
            i.name()
        ));
    }
    let names: Vec<&str> = module.exports().map(|e| e.name()).collect();
    for want in ["memory", ENTRY] {
        if !names.contains(&want) {
            return Err(format!(
                "the generator does not export {want} (use necropsy_gen_sdk::export!)"
            ));
        }
    }
    Ok(())
}

/// Runs a generator in this process. The site runs it in a child process
/// instead (see `necropsy-web generate`); this is that child's work, and what
/// `pack-tool` and the tests use.
pub fn run(wasm: &[u8], seed: [u64; 4]) -> Result<Output, String> {
    run_with(wasm, seed, MAX_FUEL)
}

/// [`run`] with a fuel limit of its own.
pub fn run_with(wasm: &[u8], seed: [u64; 4], fuel: u64) -> Result<Output, String> {
    check_module(wasm)?;
    let engine = engine();
    let module = Module::new(&engine, wasm).map_err(|e| e.to_string())?;
    let limits: StoreLimits = StoreLimitsBuilder::new()
        .memory_size(MAX_MEMORY)
        .memories(1)
        .instances(1)
        .tables(4)
        .table_elements(10_000)
        .trap_on_grow_failure(true)
        .build();
    let mut store = Store::new(&engine, limits);
    store.limiter(|l| l);
    store.set_fuel(fuel).map_err(|e| e.to_string())?;
    let linker = <Linker<StoreLimits>>::new(&engine);
    let instance = linker
        .instantiate_and_start(&mut store, &module)
        .map_err(|e| format!("the generator could not start: {e}"))?;
    let entry = instance
        .get_typed_func::<(i64, i64, i64, i64), i64>(&store, ENTRY)
        .map_err(|e| format!("{ENTRY} has the wrong signature: {e}"))?;
    let w = |i: usize| seed[i] as i64;
    let packed = entry
        .call(&mut store, (w(0), w(1), w(2), w(3)))
        .map_err(|e| {
            if e.as_trap_code() == Some(wasmi::TrapCode::OutOfFuel) {
                "the generator ran out of fuel: it took too long".to_owned()
            } else {
                format!("the generator failed: {e}")
            }
        })? as u64;
    let (ptr, len) = ((packed >> 32) as usize, (packed & 0xffff_ffff) as usize);
    if len > MAX_TOTAL + 64 * 1024 {
        return Err("the generator's output is too large".into());
    }
    let memory = instance
        .get_memory(&store, "memory")
        .ok_or("the generator has no memory")?;
    let mut buf = vec![0u8; len];
    memory
        .read(&store, ptr, &mut buf)
        .map_err(|_| "the generator's output is outside its memory".to_owned())?;
    decode(&buf)
}

/// The output in the generator's own format, to pass it from the child
/// process that ran it to the server, which [`decode`]s and checks it again.
pub fn encode(o: &Output) -> Vec<u8> {
    let mut out = b"NPGEN1".to_vec();
    let push = |out: &mut Vec<u8>, b: &[u8]| {
        out.extend_from_slice(&(b.len() as u32).to_le_bytes());
        out.extend_from_slice(b);
    };
    push(&mut out, o.answer.as_bytes());
    out.extend_from_slice(&(o.files.len() as u32).to_le_bytes());
    for f in &o.files {
        push(&mut out, f.path.as_bytes());
        push(&mut out, &f.data);
    }
    out
}

/// Seeds [`check`] tries.
pub const CHECK_SEEDS: usize = 24;

/// The pack's own text that must not hold an answer: everything but the
/// generator's code (`generator/src`, where lists of candidate values belong),
/// its build files and its module.
pub fn plaintext(files: &[(String, Vec<u8>)]) -> Vec<(String, Vec<u8>)> {
    files
        .iter()
        .filter(|(p, _)| {
            p != "generator.wasm"
                && !p.starts_with("generator/src/")
                && !p.starts_with("generator/target/")
                && !p.starts_with("generator/Cargo.")
        })
        .cloned()
        .collect()
}

/// The checks a generator must pass, over [`CHECK_SEEDS`] seeds keyed with
/// `key` (never the site's secret, so no output is a real player's):
/// - it runs within its limits and is deterministic (two runs, same output);
/// - different seeds give different answers (one repeat is allowed);
/// - no answer is in the pack's own text ([`plaintext`]);
/// - unless `answer_in_files`, no answer is in the generated files as it is.
///
/// `pack-tool check-gen` runs it, and the site on every sealed pack it opens.
pub fn check(
    p: &crate::Pack,
    plaintext: &[(String, Vec<u8>)],
    key: &[u8],
) -> Result<String, String> {
    check_with(p, plaintext, key, &mut run)
}

/// Runs a generator's wasm with a seed.
pub type Runner<'a> = dyn FnMut(&[u8], [u64; 4]) -> Result<Output, String> + 'a;

/// [`check`], running the generator with `runner`: the site passes one that
/// runs it in a child process, as it runs every generator.
pub fn check_with(
    p: &crate::Pack,
    plaintext: &[(String, Vec<u8>)],
    key: &[u8],
    runner: &mut Runner<'_>,
) -> Result<String, String> {
    let run = |w: &[u8], s: [u64; 4], r: &mut Runner<'_>| r(w, s);
    let wasm = p.generator.as_deref().ok_or("no generator")?;
    let id = &p.meta.id;
    let mut answers = std::collections::HashSet::new();
    let started = std::time::Instant::now();
    for i in 0..CHECK_SEEDS {
        let seed = seed_for(key, id, &format!("check-{i}"));
        let a = run(wasm, seed, runner)?;
        let b = run(wasm, seed, runner)?;
        if a != b {
            return Err(format!(
                "seed {i}: two runs gave different output: it is not deterministic"
            ));
        }
        let answer = crate::normalize_answer(&a.answer);
        for (path, data) in plaintext {
            if String::from_utf8_lossy(data)
                .to_lowercase()
                .contains(&answer)
            {
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

/// Whether `p` is a plain relative path: `/`-separated names, no `.` or `..`,
/// no backslashes or control characters, at most 8 levels.
pub fn valid_path(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 200
        && p.split('/').count() <= 8
        && p.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
        && !p.chars().any(|c| c == '\\' || c.is_control())
}

/// Reads a generator's output: `NPGEN1`, the answer, then each file, every
/// length a little-endian u32.
pub fn decode(b: &[u8]) -> Result<Output, String> {
    let mut at = b
        .strip_prefix(b"NPGEN1")
        .ok_or("the generator's output does not start with NPGEN1")?;
    fn take<'a>(at: &mut &'a [u8], what: &str) -> Result<&'a [u8], String> {
        if at.len() < 4 {
            return Err(format!("the generator's output ends inside {what}"));
        }
        let n = u32::from_le_bytes(at[..4].try_into().expect("4 bytes")) as usize;
        if at.len() < 4 + n {
            return Err(format!("the generator's output ends inside {what}"));
        }
        let v = &at[4..4 + n];
        *at = &at[4 + n..];
        Ok(v)
    }
    let answer = std::str::from_utf8(take(&mut at, "the answer")?)
        .map_err(|_| "the answer is not UTF-8")?
        .to_owned();
    if answer.trim().is_empty() {
        return Err("the generator set no answer".into());
    }
    if answer.len() > MAX_ANSWER {
        return Err(format!("the answer is longer than {MAX_ANSWER} bytes"));
    }
    if at.len() < 4 {
        return Err("the generator's output has no file count".into());
    }
    let count = u32::from_le_bytes(at[..4].try_into().expect("4 bytes")) as usize;
    at = &at[4..];
    if count == 0 {
        return Err("the generator made no files".into());
    }
    if count > MAX_FILES {
        return Err(format!("the generator made more than {MAX_FILES} files"));
    }
    let mut files: Vec<PackFile> = Vec::with_capacity(count);
    let mut total = 0;
    for _ in 0..count {
        let path = std::str::from_utf8(take(&mut at, "a path")?)
            .map_err(|_| "a file path is not UTF-8")?
            .to_owned();
        if !valid_path(&path) {
            return Err(format!("{path:?} is not a plain relative path"));
        }
        if files.iter().any(|f| f.path == path) {
            return Err(format!("the generator made {path} twice"));
        }
        let data = take(&mut at, "a file")?.to_vec();
        if data.len() > MAX_FILE {
            return Err(format!("{path} is larger than {MAX_FILE} bytes"));
        }
        total += data.len();
        if total > MAX_TOTAL {
            return Err(format!(
                "the files are larger than {MAX_TOTAL} bytes in all"
            ));
        }
        files.push(PackFile { path, data });
    }
    if !at.is_empty() {
        return Err("the generator's output has bytes after the last file".into());
    }
    Ok(Output { answer, files })
}

#[cfg(test)]
mod tests {
    use super::*;
    use necropsy_gen_sdk::Gen;

    fn demo(g: &mut Gen) {
        let n = g.rng.below(1_000_000);
        g.file("log.txt", format!("user u{n} logged in\n"));
        let data = g.rng.bytes(16);
        g.file("sub/data.bin", data);
        g.answer(format!("u{n}"));
    }

    #[test]
    fn decode_reads_what_the_sdk_writes() {
        let raw = necropsy_gen_sdk::run(demo, [1, 2, 3, 4]);
        let out = decode(&raw).unwrap();
        assert_eq!(encode(&out), raw, "encode is decode's inverse");
        assert!(out.answer.starts_with('u'));
        assert_eq!(out.files.len(), 2);
        assert_eq!(out.files[1].path, "sub/data.bin");
    }

    #[test]
    fn decode_refuses_bad_output() {
        fn with(answer: &str, files: &[(&str, usize)]) -> Vec<u8> {
            let mut g = Gen::new([1; 4]);
            if !answer.is_empty() {
                g.answer(answer);
            }
            for (p, n) in files {
                g.file(*p, vec![b'x'; *n]);
            }
            g.finish()
        }
        let cases: Vec<(Vec<u8>, &str)> = vec![
            (b"NOPE".to_vec(), "NPGEN1"),
            (with("", &[("a", 1)]), "no answer"),
            (with("x", &[]), "no files"),
            (with("x", &[("../etc/passwd", 1)]), "plain relative path"),
            (with("x", &[("/abs", 1)]), "plain relative path"),
            (with("x", &[("a\\b", 1)]), "plain relative path"),
            (with("x", &[("a", 1), ("a", 1)]), "twice"),
            (with("x", &[("big", MAX_FILE + 1)]), "larger than"),
            (
                with(&"a".repeat(MAX_ANSWER + 1), &[("a", 1)]),
                "longer than",
            ),
        ];
        for (bytes, want) in cases {
            let e = decode(&bytes).unwrap_err();
            assert!(e.contains(want), "{e} (wanted {want})");
        }
        let mut cut = with("x", &[("a", 10)]);
        cut.truncate(cut.len() - 3);
        assert!(decode(&cut).unwrap_err().contains("ends inside"));
        let mut extra = with("x", &[("a", 1)]);
        extra.push(0);
        assert!(decode(&extra).unwrap_err().contains("after the last file"));
    }

    #[test]
    fn seeds_depend_on_the_secret_the_pack_and_the_player() {
        let a = seed_for(b"secret", "pack", "player");
        assert_eq!(a, seed_for(b"secret", "pack", "player"));
        assert_ne!(a, seed_for(b"other", "pack", "player"));
        assert_ne!(a, seed_for(b"secret", "pack2", "player"));
        assert_ne!(a, seed_for(b"secret", "pack", "player2"));
    }

    #[test]
    fn paths() {
        assert!(valid_path("a/b.txt"));
        for bad in [
            "",
            "/a",
            "a/",
            "a//b",
            "./a",
            "a/../b",
            "a\\b",
            "a\nb",
            "1/2/3/4/5/6/7/8/9",
        ] {
            assert!(!valid_path(bad), "{bad:?}");
        }
    }

    /// Hand-written modules: the runner's refusals.
    fn wat(src: &str) -> Vec<u8> {
        wat::parse_str(src).unwrap()
    }

    #[test]
    fn modules_that_import_anything_are_refused() {
        let m = wat(
            r#"(module (import "env" "clock" (func)) (memory (export "memory") 1)
            (func (export "necropsy_generate") (param i64 i64 i64 i64) (result i64) i64.const 0))"#,
        );
        assert!(run(&m, [0; 4]).unwrap_err().contains("imports env::clock"));
        let no_entry = wat(r#"(module (memory (export "memory") 1))"#);
        assert!(run(&no_entry, [0; 4])
            .unwrap_err()
            .contains("does not export necropsy_generate"));
        assert!(run(b"not wasm", [0; 4])
            .unwrap_err()
            .contains("not a valid"));
    }

    #[test]
    fn a_loop_runs_out_of_fuel() {
        let m = wat(r#"(module (memory (export "memory") 1)
            (func (export "necropsy_generate") (param i64 i64 i64 i64) (result i64)
              (loop $l (br $l)) i64.const 0))"#);
        assert!(run_with(&m, [0; 4], 1_000_000)
            .unwrap_err()
            .contains("out of fuel"));
    }

    #[test]
    fn memory_is_capped() {
        // Ask for 2 GiB.
        let m = wat(r#"(module (memory (export "memory") 1)
            (func (export "necropsy_generate") (param i64 i64 i64 i64) (result i64)
              (drop (memory.grow (i32.const 32768))) i64.const 0))"#);
        let e = run(&m, [0; 4]).unwrap_err();
        assert!(e.contains("failed"), "{e}");
        let big_start = wat(r#"(module (memory (export "memory") 2000)
            (func (export "necropsy_generate") (param i64 i64 i64 i64) (result i64) i64.const 0))"#);
        assert!(run(&big_start, [0; 4]).is_err());
    }

    #[test]
    fn output_outside_memory_is_refused() {
        let m = wat(r#"(module (memory (export "memory") 1)
            (func (export "necropsy_generate") (param i64 i64 i64 i64) (result i64)
              i64.const 0x0000fff000001000))"#);
        assert!(run(&m, [0; 4]).unwrap_err().contains("outside its memory"));
    }
}
