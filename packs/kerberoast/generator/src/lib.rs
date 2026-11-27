// SPDX-License-Identifier: AGPL-3.0-or-later
//! Roasting: a day of 4769 events. One user asks for RC4 tickets to every
//! service account in a few seconds, from a machine that is not theirs. Every
//! player gets their own user names, workstations and attacker address.

use necropsy_gen_sdk::{export, names, template, Gen};

const TEMPLATE: &str = include_str!("../template/4769.csv");
/// The users in the template, in order. U11 is the attacker.
const USERS: usize = 15;
const ATTACKER: usize = 10;

fn generate(g: &mut Gen) {
    let users = names::distinct_users(&mut g.rng, USERS);
    // Each user keeps one workstation all day; the attacker's burst comes from
    // an address on another subnet.
    let mut hosts: Vec<u64> = (10..=250).collect();
    g.rng.shuffle(&mut hosts);
    let attacker_ip = format!(
        "10.0.{}.{}",
        g.rng.pick(&[6u64, 7, 8, 9, 11, 12]),
        g.rng.range(10, 250)
    );
    let mut values: Vec<(String, String)> = Vec::new();
    for (i, u) in users.iter().enumerate() {
        let key = if i == ATTACKER {
            "ATTACKER".to_owned()
        } else {
            format!("U{:02}", i + 1)
        };
        values.push((key, u.clone()));
        values.push((format!("W{:02}", i + 1), format!("10.0.4.{}", hosts[i])));
    }
    values.push(("ATTACKER_IP".into(), attacker_ip));
    let pairs: Vec<(&str, &str)> = values
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    g.file("4769.csv", template::render(TEMPLATE, &pairs));
    g.answer(users[ATTACKER].clone());
}

export!(generate);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_answer_is_the_user_with_the_rc4_burst() {
        for seed in 0..20u64 {
            let mut g = Gen::new([seed, 1, 2, 3]);
            generate(&mut g);
            let csv = String::from_utf8(g.file_data("4769.csv").unwrap().to_vec()).unwrap();
            assert!(!csv.contains("{{"));
            // The user with the most RC4 tickets is the answer.
            let mut counts = std::collections::HashMap::new();
            for line in csv.lines().filter(|l| l.contains(",0x17,")) {
                let user = line.split(',').nth(2).unwrap().split('@').next().unwrap();
                if !user.starts_with("svc-") {
                    *counts.entry(user.to_owned()).or_insert(0) += 1;
                }
            }
            let (top, n) = counts.into_iter().max_by_key(|(_, n)| *n).unwrap();
            assert!(n >= 10, "{n}");
            assert_eq!(g.answer_text(), Some(top.as_str()));
        }
    }
}
