// SPDX-License-Identifier: AGPL-3.0-or-later
//! Routine bump: a dependency update whose lockfile swaps the real
//! `@typescript-eslint/parser` for a lookalike scope with an install script.
//! Every player gets their own lookalike: two or three letter typos in the
//! scope name, and sometimes another separator.

use necropsy_gen_sdk::{export, template, Gen, Rng};

const TEMPLATE: &str = include_str!("../template/pr-1482.diff");
const REAL: &str = "typescript-eslint";

/// One letter typo: drop, double or swap a letter, or a lookalike character.
/// The separator is left alone.
fn letter_typo(rng: &mut Rng, s: &str) -> String {
    let mut c: Vec<char> = s.chars().collect();
    loop {
        let i = rng.below(c.len() as u64) as usize;
        if !c[i].is_ascii_lowercase() {
            continue;
        }
        match rng.below(4) {
            0 => {
                c.remove(i);
            }
            1 => c.insert(i, c[i]),
            2 if i + 1 < c.len() && c[i + 1].is_ascii_lowercase() && c[i] != c[i + 1] => {
                c.swap(i, i + 1)
            }
            3 => {
                // Lookalikes npm allows: lowercase letters and digits.
                let look = match c[i] {
                    'l' => '1',
                    'i' => 'l',
                    'o' => '0',
                    'e' => 'c',
                    'n' => 'm',
                    'r' => 'n',
                    't' => 'f',
                    _ => continue,
                };
                c[i] = look;
            }
            _ => continue,
        }
        return c.into_iter().collect();
    }
}

/// A scope two or three letter typos away from the real one, and half the
/// time with another separator.
fn lookalike(rng: &mut Rng) -> String {
    loop {
        let mut s = REAL.to_owned();
        for _ in 0..rng.range(2, 3) {
            s = letter_typo(rng, &s);
        }
        if rng.below(2) == 0 {
            s = s.replacen('-', rng.pick(&["_", "."]), 1);
        }
        // Typos can cancel out (a doubled letter dropped again): the letters
        // themselves must differ from the real scope's.
        let letters_differ = s.replace(['_', '.'], "-") != REAL;
        if letters_differ && !s.starts_with(['.', '_', '-']) && !s.ends_with(['.', '_', '-']) {
            return s;
        }
    }
}

fn generate(g: &mut Gen) {
    let scope = lookalike(&mut g.rng);
    g.file(
        "pr-1482.diff",
        template::render(TEMPLATE, &[("SQUAT", &scope)]),
    );
    g.answer(format!("@{scope}/parser"));
}

export!(generate);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_impostor_is_close_but_never_the_real_scope() {
        let mut seen = std::collections::HashSet::new();
        for seed in 0..200u64 {
            let mut g = Gen::new([seed, 3, 1, 4]);
            generate(&mut g);
            let a = g.answer_text().unwrap().to_owned();
            assert_ne!(a.replace(['_', '.'], "-"), "@typescript-eslint/parser");
            assert!(a.starts_with('@') && a.ends_with("/parser"));
            let diff = String::from_utf8(g.file_data("pr-1482.diff").unwrap().to_vec()).unwrap();
            assert_eq!(
                diff.matches(&a).count(),
                4,
                "package.json twice, the lockfile twice"
            );
            seen.insert(a);
        }
        assert!(
            seen.len() >= 195,
            "{} different impostors in 200",
            seen.len()
        );
    }
}
