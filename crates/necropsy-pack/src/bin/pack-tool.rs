// SPDX-License-Identifier: AGPL-3.0-or-later
//! Authoring helper for pack writers.
//!
//! ```text
//! pack-tool hash <pack-id> <answer>        print the value for a pack's `answer` file
//! pack-tool validate <packs-dir> [learn]   load every pack, print id, number and pack hash;
//!                                          then check the Learn modules (default: the
//!                                          `learn` directory next to the packs)
//! ```

use std::path::Path;
use std::process::ExitCode;

fn validate(dir: &str, learn: Option<&str>) -> ExitCode {
    let packs = match necropsy_pack::load_all(Path::new(dir)) {
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

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["hash", id, answer] => {
            println!("{}", necropsy_pack::hash_answer(id, answer));
            ExitCode::SUCCESS
        }
        ["validate", dir] => validate(dir, None),
        ["validate", dir, learn] => validate(dir, Some(learn)),
        _ => {
            eprintln!(
                "usage: pack-tool hash <pack-id> <answer>\n       pack-tool validate <packs-dir> [learn-dir]"
            );
            ExitCode::from(2)
        }
    }
}
