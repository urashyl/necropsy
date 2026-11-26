// SPDX-License-Identifier: AGPL-3.0-or-later
//! Live scenarios: the declarative description of a server a player talks to.
//!
//! A scenario can instead say `[container] image = "name"` and run real
//! software in a sandbox, see `docs/range.md`; it then has no routes.
//!
//! A live pack has a `live.toml` next to its `pack.toml`. The web front end
//! spins up an instance of the scenario for a player at its own subdomain and
//! answers requests from these routes. Nothing in a scenario is code: a
//! response is a status, headers and a body, and a route fires on simple
//! conditions over the request. That keeps the whole thing safe to host.
//!
//! ```toml
//! ttl_minutes = 60            # how long an instance lives
//! flag_prefix = "necropsy"    # the flag is prefix{...}, different per instance
//!
//! [[route]]
//! method = "POST"             # optional, any method if left out
//! path = "/api/sync"          # exact; a trailing * matches a prefix
//! needs = ["stage1"]          # optional: instance flags that must be set
//! sets = ["stage2"]           # optional: instance flags to set when this fires
//! [route.when]                # every condition must hold
//! header = { "user-agent" = "re:^Pebble/2\\." }
//! [route.respond]
//! status = 200
//! content_type = "application/json"
//! body = '{"ok":true,"flag":"{{flag}}"}'
//! ```
//!
//! A condition value is matched exactly, or as a regular expression when it
//! starts with `re:`, or just for presence when it is `*`. Values, paths and
//! bodies may use templates, which are filled in per instance:
//! `{{flag}}`, `{{instance}}`, `{{host}}` and `{{d:name}}`, a value derived
//! from the instance's secret seed, different for every player (credentials,
//! tokens and the like). Any of them can be wrapped in an encoding, for a value
//! hidden in obfuscated code: `{{b64:host}}`, `{{hex:flag}}`, `{{rev:d:token}}`.

use std::collections::BTreeMap;

use serde::Deserialize;

/// Longest an instance may live.
pub const MAX_TTL_MINUTES: u32 = 240;
/// Most routes in one scenario.
pub const MAX_ROUTES: usize = 200;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    #[serde(default = "default_ttl")]
    pub ttl_minutes: u32,
    #[serde(default = "default_prefix")]
    pub flag_prefix: String,
    /// Run a real, sandboxed container instead of simulated routes.
    #[serde(default)]
    pub container: Option<Container>,
    #[serde(default, rename = "route")]
    pub routes: Vec<Route>,
    /// Answers requests no route matched. Defaults to a plain 404.
    #[serde(default)]
    pub fallback: Option<Respond>,
}

fn default_ttl() -> u32 {
    60
}

fn default_prefix() -> String {
    "necropsy".into()
}

/// A real server for each instance, run by the spawner (`docs/range.md`).
/// `image` names an entry of the spawner's allowlist, `range/images.toml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Container {
    pub image: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Route {
    #[serde(default)]
    pub method: Option<String>,
    pub path: String,
    #[serde(default)]
    pub when: When,
    #[serde(default)]
    pub needs: Vec<String>,
    #[serde(default)]
    pub sets: Vec<String>,
    pub respond: Respond,
}

/// Conditions over a request. Header names are matched case-insensitively.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct When {
    #[serde(default)]
    pub header: BTreeMap<String, String>,
    #[serde(default)]
    pub query: BTreeMap<String, String>,
    #[serde(default)]
    pub cookie: BTreeMap<String, String>,
    /// Fields of a `application/x-www-form-urlencoded` body.
    #[serde(default)]
    pub form: BTreeMap<String, String>,
    /// The whole request body.
    #[serde(default)]
    pub body: Option<String>,
    /// `user:password` the request must present with HTTP Basic auth.
    #[serde(default)]
    pub basic_auth: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Respond {
    #[serde(default = "default_status")]
    pub status: u16,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Inline body, templated. Exclusive with `file`.
    #[serde(default)]
    pub body: Option<String>,
    /// A file under the pack's `live/` directory, sent as is (not templated),
    /// so it can be a real sample.
    #[serde(default)]
    pub file: Option<String>,
}

fn default_status() -> u16 {
    200
}

impl Scenario {
    /// Parses and checks a `live.toml`. `has_file` says whether a path exists
    /// under the pack's `live/` directory.
    pub fn parse(text: &str, has_file: impl Fn(&str) -> bool) -> Result<Self, String> {
        let s: Scenario = toml::from_str(text).map_err(|e| e.to_string())?;
        if s.ttl_minutes == 0 || s.ttl_minutes > MAX_TTL_MINUTES {
            return Err(format!("ttl_minutes must be 1 to {MAX_TTL_MINUTES}"));
        }
        if s.flag_prefix.is_empty()
            || s.flag_prefix.len() > 24
            || !s
                .flag_prefix
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        {
            return Err("flag_prefix must be 1 to 24 characters of a-z and 0-9".into());
        }
        if let Some(c) = &s.container {
            if !s.routes.is_empty() || s.fallback.is_some() {
                return Err("a container scenario has no routes or fallback".into());
            }
            let ok = !c.image.is_empty()
                && c.image.len() <= 40
                && c.image
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
            if !ok {
                return Err(format!("bad container image name {:?}", c.image));
            }
            return Ok(s);
        }
        if s.routes.is_empty() || s.routes.len() > MAX_ROUTES {
            return Err(format!("a scenario needs 1 to {MAX_ROUTES} routes"));
        }
        for r in &s.routes {
            if !r.path.starts_with('/') {
                return Err(format!("route path {:?} must start with /", r.path));
            }
            if let Some(m) = &r.method {
                if !["GET", "POST", "PUT", "DELETE", "HEAD", "PATCH", "OPTIONS"]
                    .contains(&m.as_str())
                {
                    return Err(format!("unknown method {m:?} (use upper case)"));
                }
            }
            for flag in r.needs.iter().chain(&r.sets) {
                if flag.is_empty() || flag.len() > 32 || flag.contains(',') {
                    return Err(format!("bad state flag {flag:?}"));
                }
            }
            let w = &r.when;
            let values = w
                .header
                .values()
                .chain(w.query.values())
                .chain(w.cookie.values())
                .chain(w.form.values())
                .chain(w.body.iter());
            for v in values {
                check_matcher(v)?;
            }
            check_respond(&r.respond, &has_file)?;
        }
        if let Some(f) = &s.fallback {
            check_respond(f, &has_file)?;
        }
        Ok(s)
    }
}

fn check_matcher(v: &str) -> Result<(), String> {
    // A pattern with a template can only be compiled once it is filled in.
    if let Some(re) = v.strip_prefix("re:") {
        if !re.contains("{{") {
            regex::Regex::new(re).map_err(|e| format!("bad pattern {v:?}: {e}"))?;
        }
    }
    Ok(())
}

fn check_respond(r: &Respond, has_file: &impl Fn(&str) -> bool) -> Result<(), String> {
    if !(100..=599).contains(&r.status) {
        return Err(format!("bad status {}", r.status));
    }
    match (&r.body, &r.file) {
        (Some(_), Some(_)) => return Err("a response has a body or a file, not both".into()),
        (None, Some(f)) if !has_file(f) => {
            return Err(format!("file {f:?} is not in the pack's live/ directory"))
        }
        _ => {}
    }
    for name in r.headers.keys() {
        let n = name.to_ascii_lowercase();
        if matches!(
            n.as_str(),
            "content-length" | "transfer-encoding" | "connection"
        ) {
            return Err(format!("header {name} is set by the server"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(t: &str) -> Result<Scenario, String> {
        Scenario::parse(t, |f| f == "ok.txt")
    }

    #[test]
    fn a_minimal_scenario_parses_with_defaults() {
        let s = parse("[[route]]\npath = \"/\"\n[route.respond]\nbody = \"hi\"").unwrap();
        assert_eq!((s.ttl_minutes, s.flag_prefix.as_str()), (60, "necropsy"));
        assert_eq!(s.routes[0].respond.status, 200);
    }

    #[test]
    fn mistakes_are_caught_at_load_time() {
        let route = |extra: &str, respond: &str| {
            format!("[[route]]\npath = \"/x\"\n{extra}\n[route.respond]\n{respond}")
        };
        for (why, text) in [
            ("no routes", "ttl_minutes = 5".to_owned()),
            (
                "ttl too long",
                format!("ttl_minutes = 999\n{}", route("", "")),
            ),
            (
                "bad prefix",
                format!("flag_prefix = \"A B\"\n{}", route("", "")),
            ),
            (
                "relative path",
                "[[route]]\npath = \"x\"\n[route.respond]\n".to_owned(),
            ),
            ("bad method", route("method = \"get\"", "")),
            (
                "bad regex",
                route("[route.when]\nheader = { a = \"re:(\" }", ""),
            ),
            ("missing file", route("", "file = \"nope.txt\"")),
            (
                "body and file",
                route("", "body = \"a\"\nfile = \"ok.txt\""),
            ),
            ("bad status", route("", "status = 99")),
            (
                "server header",
                route("", "headers = { content-length = \"1\" }"),
            ),
            ("unknown key", route("surprise = 1", "")),
            (
                "container with routes",
                format!("[container]\nimage = \"lab\"\n{}", route("", "")),
            ),
            ("bad image name", "[container]\nimage = \"-x y\"".to_owned()),
        ] {
            assert!(parse(&text).is_err(), "{why} should be rejected");
        }
        let c = parse("[container]\nimage = \"lab\"").unwrap();
        assert_eq!(c.container.unwrap().image, "lab");
        // A pattern with a template is only compiled once it is filled in.
        assert!(parse(&route(
            "[route.when]\nheader = { a = \"re:^{{d:x}}(\" }",
            ""
        ))
        .is_ok());
    }
}
