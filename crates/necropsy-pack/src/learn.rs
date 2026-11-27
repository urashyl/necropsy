// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Learn track: modules that take a beginner to the level of a 5/5 daily.
//! See `docs/learn.md`.
//!
//! ```text
//! learn/
//!   01-reading-a-pcap/
//!     module.toml   id, number, tier, title, summary, practice packs, checkpoint
//!     concept.md    the idea, tool-independent
//!     browser.md    walkthrough with the in-browser workspace
//!     terminal.md   walkthrough with local tools
//! ```

use std::collections::HashSet;
use std::path::Path;

use serde::Deserialize;

use crate::{curl, invalid, normalize_answer, Pack, PackError};

/// How a checkpoint answer is compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Normalized like a puzzle answer: case, surrounding quotes and runs of
    /// whitespace do not matter.
    #[default]
    Text,
    /// A Wireshark display filter: also ignores spaces and quotes, and treats
    /// `eq` as `==`, `and` as `&&`, `or` as `||`.
    Filter,
    /// A curl command that must send the request in `curl`.
    Curl,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Question {
    pub question: String,
    #[serde(default)]
    pub kind: Kind,
    /// Accepted answers, for `text` and `filter`.
    #[serde(default)]
    pub accept: Vec<String>,
    /// The request a `curl` answer must send, written as a curl command.
    #[serde(default)]
    pub curl: Option<String>,
    /// Shown after a wrong answer.
    #[serde(default)]
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleMeta {
    /// Slug, used in URLs: `/learn/<id>`.
    pub id: String,
    pub number: u32,
    /// The daily difficulty this module prepares for, 1 to 5.
    pub tier: u8,
    pub title: String,
    pub summary: String,
    /// Practice puzzles, by pack id, in order. The first is the module's own.
    pub practice: Vec<String>,
    /// More puzzles at this level, by pack id.
    #[serde(default)]
    pub more: Vec<String>,
    pub checkpoint: Vec<Question>,
}

#[derive(Debug, Clone)]
pub struct Module {
    pub meta: ModuleMeta,
    pub concept_md: String,
    pub browser_md: String,
    pub terminal_md: String,
}

/// Comparable form of a display filter.
pub fn normalize_filter(raw: &str) -> String {
    let lower = format!(" {} ", raw.trim().to_lowercase());
    let lower = lower
        .replace(" eq ", " == ")
        .replace(" ne ", " != ")
        .replace(" and ", " && ")
        .replace(" or ", " || ");
    lower
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '"' && *c != '\'')
        .collect()
}

impl Question {
    /// Whether `answer` is right.
    pub fn check(&self, answer: &str) -> bool {
        match self.kind {
            Kind::Text => {
                let a = normalize_answer(answer);
                !a.is_empty() && self.accept.iter().any(|x| normalize_answer(x) == a)
            }
            Kind::Filter => {
                let a = normalize_filter(answer);
                !a.is_empty() && self.accept.iter().any(|x| normalize_filter(x) == a)
            }
            Kind::Curl => {
                let (Some(want), Ok(got)) = (self.curl.as_deref(), curl::parse(answer.trim()))
                else {
                    return false;
                };
                curl::parse(want).is_ok_and(|w| got.sends(&w))
            }
        }
    }
}

impl Module {
    /// Grades a checkpoint: one result per question. Missing answers are wrong.
    pub fn grade(&self, answers: &[String]) -> Vec<bool> {
        self.meta
            .checkpoint
            .iter()
            .enumerate()
            .map(|(i, q)| {
                answers
                    .get(i)
                    .is_some_and(|a| a.len() <= 2048 && q.check(a))
            })
            .collect()
    }
}

fn read(dir: &Path, name: &str) -> Result<String, PackError> {
    let p = dir.join(name);
    std::fs::read_to_string(&p).map_err(|e| invalid(&p, e.to_string()))
}

fn load_module(dir: &Path) -> Result<Module, PackError> {
    let meta: ModuleMeta = toml::from_str(&read(dir, "module.toml")?)
        .map_err(|e| invalid(&dir.join("module.toml"), e.to_string()))?;
    let at = dir.join("module.toml");
    if !crate::valid_id(&meta.id) {
        return Err(invalid(&at, "id must be 1-40 chars of a-z, 0-9 and '-'"));
    }
    if !(1..=5).contains(&meta.tier) {
        return Err(invalid(&at, "tier must be 1-5"));
    }
    if meta.practice.is_empty() {
        return Err(invalid(&at, "needs at least one practice puzzle"));
    }
    if meta.checkpoint.is_empty() {
        return Err(invalid(
            &at,
            "needs a checkpoint with at least one question",
        ));
    }
    for (i, q) in meta.checkpoint.iter().enumerate() {
        let n = i + 1;
        match q.kind {
            Kind::Text | Kind::Filter if q.accept.iter().all(|a| a.trim().is_empty()) => {
                return Err(invalid(&at, format!("question {n} accepts no answer")));
            }
            Kind::Curl => {
                let want = q.curl.as_deref().ok_or_else(|| {
                    invalid(&at, format!("question {n} is a curl question with no curl"))
                })?;
                curl::parse(want).map_err(|e| {
                    invalid(&at, format!("question {n}'s curl does not parse: {e}"))
                })?;
            }
            _ => {}
        }
        if q.kind != Kind::Curl && q.curl.is_some() {
            return Err(invalid(
                &at,
                format!("question {n} has curl but is not a curl question"),
            ));
        }
    }
    let module = Module {
        meta,
        concept_md: read(dir, "concept.md")?,
        browser_md: read(dir, "browser.md")?,
        terminal_md: read(dir, "terminal.md")?,
    };
    for (name, text) in [
        ("concept.md", &module.concept_md),
        ("browser.md", &module.browser_md),
        ("terminal.md", &module.terminal_md),
    ] {
        if text.trim().is_empty() {
            return Err(invalid(&dir.join(name), "is empty"));
        }
    }
    Ok(module)
}

/// Loads every module under `root`, ordered by number, and checks that their
/// practice puzzles are packs in `packs`. A missing `root` is no modules.
pub fn load_all(root: &Path, packs: &[Pack]) -> Result<Vec<Module>, PackError> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Ok(Vec::new());
    };
    let mut dirs: Vec<_> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect();
    dirs.sort();
    let mut modules = Vec::new();
    for d in dirs {
        modules.push(load_module(&d)?);
    }
    modules.sort_by_key(|m| m.meta.number);
    let ids: HashSet<&str> = packs.iter().map(|p| p.meta.id.as_str()).collect();
    let mut seen_id = HashSet::new();
    let mut seen_num = HashSet::new();
    for m in &modules {
        let at = root.join(&m.meta.id);
        if !seen_id.insert(m.meta.id.clone()) {
            return Err(invalid(
                &at,
                format!("two modules have the id {}", m.meta.id),
            ));
        }
        if !seen_num.insert(m.meta.number) {
            return Err(invalid(
                &at,
                format!("two modules have the number {}", m.meta.number),
            ));
        }
        for p in m.meta.practice.iter().chain(&m.meta.more) {
            if !ids.contains(p.as_str()) {
                return Err(invalid(&at, format!("practice puzzle {p} is not a pack")));
            }
        }
        // A daily is hidden until its date, so it cannot be practice. A closed
        // daily joins a module through its own `learn` list instead.
        for p in packs.iter().filter(|p| {
            m.meta
                .practice
                .iter()
                .chain(&m.meta.more)
                .any(|id| *id == p.meta.id)
        }) {
            if p.meta.track == crate::Track::Daily {
                return Err(invalid(
                    &at,
                    format!(
                        "{} is a daily: practice puzzles are intro or library packs",
                        p.meta.id
                    ),
                ));
            }
        }
    }
    // Tiers never go down: the track is a path from 1 to 5.
    for w in modules.windows(2) {
        if w[1].meta.tier < w[0].meta.tier {
            return Err(invalid(
                &root.join(&w[1].meta.id),
                "modules must be in tier order: a later module has a lower tier",
            ));
        }
    }
    // A pack's `learn` list names modules that exist.
    let module_ids: HashSet<&str> = modules.iter().map(|m| m.meta.id.as_str()).collect();
    if !modules.is_empty() {
        for p in packs {
            for l in &p.meta.learn {
                if !module_ids.contains(l.as_str()) {
                    return Err(invalid(
                        Path::new(&p.meta.id),
                        format!("learn names {l}, which is not a Learn module"),
                    ));
                }
            }
        }
    }
    Ok(modules)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(kind: Kind, accept: &[&str], curl: Option<&str>) -> Question {
        Question {
            question: "?".into(),
            kind,
            accept: accept.iter().map(|s| s.to_string()).collect(),
            curl: curl.map(str::to_owned),
            hint: None,
        }
    }

    #[test]
    fn text_answers_are_normalized() {
        let t = q(Kind::Text, &["dl.example.test"], None);
        assert!(t.check("  DL.example.test "));
        assert!(t.check("`dl.example.test`"));
        assert!(!t.check("example.test"));
        assert!(!t.check(""));
    }

    #[test]
    fn filters_ignore_spacing_quotes_and_word_operators() {
        let f = q(Kind::Filter, &[r#"http.request.method == "POST""#], None);
        assert!(f.check("http.request.method==POST"));
        assert!(f.check("http.request.method eq 'POST'"));
        assert!(!f.check("http.request.method == GET"));
        let g = q(Kind::Filter, &["http && ip.dst == 10.0.0.1"], None);
        assert!(g.check("http and ip.dst eq 10.0.0.1"));
    }

    #[test]
    fn curl_answers_send_the_request() {
        let c = q(
            Kind::Curl,
            &[],
            Some("curl -H 'X-A: 1' -d 'k=v' https://h.test/p"),
        );
        assert!(c.check("curl --data k=v -H 'x-a: 1' https://h.test/p"));
        assert!(!c.check("curl -H 'X-A: 1' https://h.test/p"));
        assert!(!c.check("not curl"));
    }

    #[test]
    fn the_repository_modules_load() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let packs = crate::load_all(&root.join("packs")).unwrap();
        let modules = load_all(&root.join("learn"), &packs).unwrap();
        assert!(modules.len() >= 9, "{}", modules.len());
        assert_eq!(modules.first().unwrap().meta.tier, 1);
        assert_eq!(modules.last().unwrap().meta.tier, 5);
        for m in &modules {
            // Each question's own first accepted answer is right.
            for qn in &m.meta.checkpoint {
                if qn.kind != Kind::Curl {
                    assert!(qn.check(&qn.accept[0]), "{}: {}", m.meta.id, qn.question);
                } else {
                    assert!(qn.check(qn.curl.as_deref().unwrap()), "{}", m.meta.id);
                }
            }
        }
    }
}
