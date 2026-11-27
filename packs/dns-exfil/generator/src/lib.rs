// SPDX-License-Identifier: AGPL-3.0-or-later
//! Small packets: a resolver's query log in which the backup server sends an
//! rclone config out over DNS, base32 in the query names. Every player's config
//! has its own access key and secret, so the chunks differ.

use necropsy_gen_sdk::{encode, export, template, Gen};

const TEMPLATE: &str = include_str!("../template/queries.log");
/// Characters per chunk, and the chunks the template has room for.
const CHUNK: usize = 48;
const CHUNKS: usize = 9;

const UPPER_DIGITS: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
const B64ISH: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn generate(g: &mut Gen) {
    // The same lengths as the template's queries were made for: 19 and 40.
    let key_id = format!("AKXX{}", g.rng.string(UPPER_DIGITS, 15));
    let secret = g.rng.string(B64ISH, 40);
    let conf = format!(
        "path=/home/svc-backup/.config/rclone/rclone.conf\n[offsite]\ntype = s3\nprovider = Other\nenv_auth = false\naccess_key_id = {key_id}\nsecret_access_key = {secret}\nendpoint = https://s3.backup-vendor.invalid\n"
    );
    let data = encode::base32(conf.as_bytes());
    let chunks: Vec<String> = data
        .as_bytes()
        .chunks(CHUNK)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect();
    assert_eq!(
        chunks.len(),
        CHUNKS,
        "the config no longer fits the template's queries"
    );
    let names: Vec<String> = (0..CHUNKS).map(|i| format!("C{i:02}")).collect();
    let pairs: Vec<(&str, &str)> = names
        .iter()
        .zip(&chunks)
        .map(|(n, c)| (n.as_str(), c.as_str()))
        .collect();
    g.file("queries.log", template::render(TEMPLATE, &pairs));
    g.answer(key_id);
}

export!(generate);

#[cfg(test)]
mod tests {
    use super::*;

    fn b32decode(s: &str) -> Vec<u8> {
        let mut bits: u64 = 0;
        let mut n = 0;
        let mut out = Vec::new();
        for c in s.bytes() {
            let v = match c {
                b'a'..=b'z' => c - b'a',
                b'2'..=b'7' => c - b'2' + 26,
                _ => panic!("not base32: {c}"),
            };
            bits = (bits << 5) | u64::from(v);
            n += 5;
            if n >= 8 {
                n -= 8;
                out.push((bits >> n) as u8);
            }
        }
        out
    }

    #[test]
    fn the_queries_decode_to_the_answer() {
        for seed in 0..20u64 {
            let mut g = Gen::new([seed, 0, 0, 9]);
            generate(&mut g);
            let log = String::from_utf8(g.file_data("queries.log").unwrap().to_vec()).unwrap();
            assert!(!log.contains("{{"));
            // As the explainer does it: unique chunks by sequence number, without ff.
            let mut chunks = std::collections::BTreeMap::new();
            for name in log
                .split_whitespace()
                .filter(|w| w.ends_with(".q7f2.t.cdn-metrics.invalid"))
            {
                let mut parts = name.split('.');
                let (seq, data) = (parts.next().unwrap(), parts.next().unwrap());
                if seq != "ff" {
                    chunks.insert(seq.to_owned(), data.to_owned());
                }
            }
            let joined: String = chunks.values().cloned().collect();
            let conf = String::from_utf8(b32decode(&joined)).unwrap();
            let answer = g.answer_text().unwrap();
            assert!(
                conf.contains(&format!("access_key_id = {answer}\n")),
                "{conf}"
            );
            assert!(!log.contains(answer), "the key is only there encoded");
        }
    }
}
