// SPDX-License-Identifier: AGPL-3.0-or-later
//! One password, everyone: a day of cloud sign-ins with a password spray in
//! it. One sprayed account signs in over SMTP, which cannot do MFA. Every player
//! gets their own account names, so the compromised one differs.

use necropsy_gen_sdk::{export, names, template, Gen};

const TEMPLATE: &str = include_str!("../template/signins.jsonl");
/// Accounts in the template: U01 to U40 (one of which is not used) and VICTIM.
const ACCOUNTS: usize = 40;

fn generate(g: &mut Gen) {
    let users = names::distinct_users(&mut g.rng, ACCOUNTS + 1);
    let mut values: Vec<(String, String)> = users[..ACCOUNTS]
        .iter()
        .enumerate()
        .map(|(i, u)| (format!("U{:02}", i + 1), u.clone()))
        .collect();
    let victim = users[ACCOUNTS].clone();
    values.push(("VICTIM".into(), victim.clone()));
    let pairs: Vec<(&str, &str)> = values
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    g.file("signins.jsonl", template::render(TEMPLATE, &pairs));
    g.answer(format!("{victim}@example.invalid"));
}

export!(generate);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_answer_is_the_only_successful_legacy_sign_in() {
        for seed in 0..20u64 {
            let mut g = Gen::new([seed, 7, 7, 7]);
            generate(&mut g);
            let log = String::from_utf8(g.file_data("signins.jsonl").unwrap().to_vec()).unwrap();
            assert!(!log.contains("{{"));
            let hits: Vec<&str> = log
                .lines()
                .filter(|l| l.contains("\"BAV2ROPC\"") && l.contains("\"errorCode\": 0,"))
                .collect();
            assert!(!hits.is_empty());
            let answer = g.answer_text().unwrap();
            assert!(hits.iter().all(|l| l.contains(answer)), "{answer}");
        }
    }
}
