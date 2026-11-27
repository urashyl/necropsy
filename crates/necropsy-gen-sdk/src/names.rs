// SPDX-License-Identifier: AGPL-3.0-or-later
//! Made-up people for logs: user names such as `k.morgan`.

use crate::Rng;

/// Surnames, common across many countries, so logs look like a real company.
pub const SURNAMES: &[&str] = &[
    "abara",
    "adams",
    "ahmed",
    "alvarez",
    "amari",
    "andersen",
    "arslan",
    "bailey",
    "bakker",
    "banerjee",
    "barros",
    "bauer",
    "becker",
    "berg",
    "bianchi",
    "blom",
    "boateng",
    "brandt",
    "brooks",
    "bruno",
    "campbell",
    "carter",
    "castro",
    "chen",
    "chowdhury",
    "clarke",
    "cohen",
    "costa",
    "cruz",
    "dahl",
    "dang",
    "davies",
    "delgado",
    "diaz",
    "dimitrov",
    "dubois",
    "duarte",
    "eriksen",
    "evans",
    "farah",
    "ferrari",
    "fischer",
    "fleming",
    "fontaine",
    "fox",
    "garcia",
    "gill",
    "gomez",
    "gordon",
    "graham",
    "grant",
    "gray",
    "greco",
    "gunn",
    "haas",
    "hall",
    "hansen",
    "harris",
    "hayes",
    "hoang",
    "holm",
    "hughes",
    "hussain",
    "ibrahim",
    "ito",
    "ivanov",
    "jansen",
    "jensen",
    "jones",
    "kaya",
    "keller",
    "khan",
    "kim",
    "klein",
    "koch",
    "kowalski",
    "kruger",
    "kumar",
    "lambert",
    "lang",
    "larsen",
    "lee",
    "lehmann",
    "lewis",
    "lima",
    "lindqvist",
    "lopez",
    "lund",
    "maier",
    "malik",
    "marsh",
    "martin",
    "mason",
    "meyer",
    "mills",
    "mishra",
    "moore",
    "moreau",
    "morgan",
    "mueller",
    "murphy",
    "nagy",
    "nakamura",
    "navarro",
    "nguyen",
    "nielsen",
    "novak",
    "nowak",
    "obi",
    "okafor",
    "okoro",
    "olsen",
    "ortiz",
    "owens",
    "park",
    "patel",
    "pereira",
    "perez",
    "petrov",
    "pham",
    "popescu",
    "price",
    "quinn",
    "rahman",
    "ramos",
    "reed",
    "reid",
    "reyes",
    "richter",
    "rivera",
    "roberts",
    "rossi",
    "roux",
    "russo",
    "saito",
    "santos",
    "sato",
    "schmidt",
    "shah",
    "silva",
    "singh",
    "sousa",
    "stone",
    "suzuki",
    "svensson",
    "tanaka",
    "taylor",
    "torres",
    "tran",
    "turner",
    "vargas",
    "vega",
    "vogel",
    "wagner",
    "walsh",
    "wang",
    "ward",
    "weber",
    "west",
    "wolf",
    "wong",
    "wright",
    "yamamoto",
    "yildiz",
    "young",
    "zhang",
    "zhou",
    "ziegler",
];

/// A user name: an initial, a dot and a surname.
pub fn user(rng: &mut Rng) -> String {
    let initial = rng.string("abcdefghijklmnopqrstuvwxyz", 1);
    format!("{initial}.{}", rng.pick(SURNAMES))
}

/// `n` different user names.
pub fn distinct_users(rng: &mut Rng, n: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(n);
    while out.len() < n {
        let u = user(rng);
        if !out.contains(&u) {
            out.push(u);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinct_and_well_formed() {
        let mut r = Rng::new([3; 4]);
        let u = distinct_users(&mut r, 50);
        assert_eq!(u.len(), 50);
        let mut s = u.clone();
        s.sort();
        s.dedup();
        assert_eq!(s.len(), 50);
        assert!(u.iter().all(|n| n.len() > 2 && n.as_bytes()[1] == b'.'));
    }
}
