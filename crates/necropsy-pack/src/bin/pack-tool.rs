// SPDX-License-Identifier: AGPL-3.0-or-later
//! Authoring helper for pack writers.
//!
//! ```text
//! pack-tool hash <pack-id> <answer>   print the value for a pack's `answer` file
//! pack-tool validate <packs-dir>      load every pack, print id, number and pack hash
//! ```

use std::path::Path;
use std::process::ExitCode;

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
        ["validate", dir] => match necropsy_pack::load_all(Path::new(dir)) {
            Ok(packs) => {
                for p in packs {
                    println!("{:>3}  {:<24} {}", p.meta.number, p.meta.id, p.pack_hash);
                }
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        _ => {
            eprintln!(
                "usage: pack-tool hash <pack-id> <answer>\n       pack-tool validate <packs-dir>"
            );
            ExitCode::from(2)
        }
    }
}
