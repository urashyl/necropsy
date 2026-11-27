// SPDX-License-Identifier: AGPL-3.0-or-later
//! The generator child process, `necropsy-web generate`, as the server runs it:
//! seed and module on stdin, output on stdout, an empty environment.

use std::io::Write;
use std::process::{Command, Stdio};

use necropsy_pack::gen;

fn kerberoast() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../packs/kerberoast/generator.wasm"
    ))
    .unwrap()
}

fn run(input: &[u8]) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_necropsy-web"))
        .arg("generate")
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

fn input(seed: [u64; 4], wasm: &[u8]) -> Vec<u8> {
    let mut v: Vec<u8> = seed.iter().flat_map(|w| w.to_le_bytes()).collect();
    v.extend_from_slice(wasm);
    v
}

#[test]
fn the_child_runs_a_generator_like_the_library_does() {
    let wasm = kerberoast();
    let seed = gen::seed_for(b"k", "kerberoast", "p");
    let out = run(&input(seed, &wasm));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let from_child = gen::decode(&out.stdout).unwrap();
    assert_eq!(from_child, gen::run(&wasm, seed).unwrap());
}

#[test]
fn the_child_refuses_bad_input_and_says_why() {
    let out = run(b"short");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("expected a seed"));
    let out = run(&input([1; 4], b"not wasm at all"));
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not a valid WebAssembly module"));
    assert!(out.stdout.is_empty());
}
