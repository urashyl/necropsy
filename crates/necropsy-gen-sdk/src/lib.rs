// SPDX-License-Identifier: AGPL-3.0-or-later
//! Writing a pack generator. See `docs/generators.md`.
//!
//! A generator is a function from a seed to a player's files and expected answer.
//! It is compiled to WebAssembly (`wasm32-unknown-unknown`) and run by the site
//! with no imports at all: no clock, no randomness but the seed, no files, no
//! network. So the same seed always gives the same files and answer.
//!
//! ```ignore
//! use necropsy_gen_sdk::{export, Gen};
//!
//! fn generate(g: &mut Gen) {
//!     let user = g.rng.pick(&["k.morgan", "a.ito", "j.diaz"]).to_string();
//!     g.file("log.txt", format!("login ok user={user}\n"));
//!     g.answer(&user);
//! }
//! export!(generate);
//! ```

pub mod encode;
pub mod names;
pub mod pcap;
mod rng;
pub mod template;

pub use rng::Rng;

/// What a generator builds for one seed.
pub struct Gen {
    /// Randomness from the seed, and nothing else.
    pub rng: Rng,
    files: Vec<(String, Vec<u8>)>,
    answer: Option<String>,
}

impl Gen {
    pub fn new(seed: [u64; 4]) -> Self {
        Self {
            rng: Rng::new(seed),
            files: Vec::new(),
            answer: None,
        }
    }

    /// Adds a file the player gets, at `path` under the pack's `files/`.
    pub fn file(&mut self, path: impl Into<String>, data: impl Into<Vec<u8>>) {
        self.files.push((path.into(), data.into()));
    }

    /// Sets the answer this player's files lead to.
    pub fn answer(&mut self, answer: impl Into<String>) {
        self.answer = Some(answer.into());
    }

    /// The answer set so far, for a generator's own tests.
    pub fn answer_text(&self) -> Option<&str> {
        self.answer.as_deref()
    }

    /// A file added so far, for a generator's own tests.
    pub fn file_data(&self, path: &str) -> Option<&[u8]> {
        self.files
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, d)| d.as_slice())
    }

    /// The output in the format the site reads: `NPGEN1`, then the answer,
    /// the file count, and each path and its bytes, each length a u32 (LE).
    pub fn finish(self) -> Vec<u8> {
        let mut out = b"NPGEN1".to_vec();
        let push = |out: &mut Vec<u8>, b: &[u8]| {
            out.extend_from_slice(&(b.len() as u32).to_le_bytes());
            out.extend_from_slice(b);
        };
        push(&mut out, self.answer.unwrap_or_default().as_bytes());
        out.extend_from_slice(&(self.files.len() as u32).to_le_bytes());
        for (p, d) in &self.files {
            push(&mut out, p.as_bytes());
            push(&mut out, d);
        }
        out
    }
}

/// Runs a generator function on a seed and returns its encoded output. The
/// [`export!`] macro calls this; tests can call it directly.
pub fn run(f: fn(&mut Gen), seed: [u64; 4]) -> Vec<u8> {
    let mut g = Gen::new(seed);
    f(&mut g);
    g.finish()
}

/// Exports a generator function as the module's entry point, `necropsy_generate`.
/// The site calls it with the seed as four integers and reads the output from
/// memory at the returned pointer (high 32 bits) and length (low 32 bits).
#[macro_export]
macro_rules! export {
    ($f:path) => {
        #[no_mangle]
        pub extern "C" fn necropsy_generate(a: u64, b: u64, c: u64, d: u64) -> u64 {
            let out = $crate::run($f, [a, b, c, d]).into_boxed_slice();
            let len = out.len() as u64;
            let ptr = ::std::boxed::Box::leak(out).as_ptr() as usize as u64;
            (ptr << 32) | len
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo(g: &mut Gen) {
        let n = g.rng.below(1000);
        g.file("a.txt", format!("{n}\n"));
        g.file("dir/b.bin", vec![0u8, 1, 2]);
        g.answer(format!("n{n}"));
    }

    #[test]
    fn output_format() {
        let out = run(demo, [1, 2, 3, 4]);
        assert!(out.starts_with(b"NPGEN1"));
        assert_eq!(out, run(demo, [1, 2, 3, 4]), "same seed, same bytes");
        assert_ne!(out, run(demo, [1, 2, 3, 5]));
        // answer length, then the answer
        let alen = u32::from_le_bytes(out[6..10].try_into().unwrap()) as usize;
        assert!(std::str::from_utf8(&out[10..10 + alen])
            .unwrap()
            .starts_with('n'));
        let count = u32::from_le_bytes(out[10 + alen..14 + alen].try_into().unwrap());
        assert_eq!(count, 2);
    }
}
