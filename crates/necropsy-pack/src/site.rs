// SPDX-License-Identifier: AGPL-3.0-or-later
//! The site `pack-tool` asks for keys: the wrap key to seal to, and the keys of
//! released packs. See `docs/sealing.md`.
//!
//! Which site: `--site <url>` if given; else `NECROPSY_BASE_URL` from the
//! environment; else from a `.env` file in the current directory; else
//! [`DEFAULT_SITE`]. So an author with a local development site and its own
//! `.env` seals to their dev key, and anyone else seals to production's.

use std::path::Path;

use crate::seal::WrapKey;

/// Where packs are sealed to when nothing says otherwise.
pub const DEFAULT_SITE: &str = "https://necropsy.dev";
/// The site's endpoints.
pub const WRAP_KEY_PATH: &str = "/api/packs/wrap-key";

/// The value of `key` in a `.env`-style file: `KEY=value`, optionally quoted,
/// `export ` allowed, `#` comments skipped. The last one wins.
pub fn env_file_value(text: &str, key: &str) -> Option<String> {
    let mut found = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        if k.trim() != key {
            continue;
        }
        let v = v.trim();
        let v = v
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .or_else(|| v.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
            .unwrap_or(v);
        if !v.is_empty() {
            found = Some(v.to_owned());
        }
    }
    found
}

/// The site to use, and where that choice came from.
pub fn resolve(
    flag: Option<&str>,
    env: Option<String>,
    env_file: Option<&Path>,
) -> (String, &'static str) {
    let clean = |s: &str| s.trim().trim_end_matches('/').to_owned();
    if let Some(f) = flag.filter(|f| !f.trim().is_empty()) {
        return (clean(f), "--site");
    }
    if let Some(e) = env.filter(|e| !e.trim().is_empty()) {
        return (clean(&e), "NECROPSY_BASE_URL");
    }
    if let Some(v) = env_file
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| env_file_value(&t, "NECROPSY_BASE_URL"))
    {
        return (clean(&v), ".env");
    }
    (DEFAULT_SITE.to_owned(), "the default")
}

/// Whether a URL is on this machine: a local development site, which is never
/// reached through a proxy.
fn is_loopback(url: &str) -> bool {
    let host = url
        .split_once("://")
        .map_or(url, |(_, r)| r)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("");
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    let host = if host.starts_with('[') {
        host.split(']').next().unwrap_or("").trim_start_matches('[')
    } else {
        host.split(':').next().unwrap_or("")
    };
    host.eq_ignore_ascii_case("localhost")
        || host.ends_with(".localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

fn get_json(url: &str) -> Result<serde_json::Value, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(20))
        .try_proxy_from_env(!is_loopback(url))
        .build();
    match agent.get(url).call() {
        Ok(res) => res.into_json().map_err(|e| format!("{url}: not JSON: {e}")),
        Err(ureq::Error::Status(code, res)) => {
            let body: String = res
                .into_string()
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect();
            Err(format!("{url} answered {code}: {}", body.trim()))
        }
        Err(e) => Err(format!("cannot reach {url}: {e}")),
    }
}

/// The wrap key a site seals to now.
pub fn fetch_wrap_key(site: &str) -> Result<WrapKey, String> {
    let url = format!("{site}{WRAP_KEY_PATH}");
    WrapKey::from_json(&get_json(&url)?).map_err(|e| format!("{url}: {e}"))
}

/// A released pack's key, from a site.
pub fn fetch_released_key(site: &str, id: &str) -> Result<[u8; 32], String> {
    let url = format!("{site}/api/packs/{id}/key");
    let v = get_json(&url)?;
    v["key"]
        .as_str()
        .and_then(crate::seal::key_from_hex)
        .ok_or_else(|| format!("{url}: no key in the answer"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flag_then_the_environment_then_dot_env_then_the_default() {
        let dir = std::env::temp_dir().join(format!("necropsy-site-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let env_file = dir.join(".env");
        std::fs::write(
            &env_file,
            "# local\nexport NECROPSY_BASE_URL='http://localhost:8080/'\n",
        )
        .unwrap();
        assert_eq!(
            resolve(
                Some("https://dev.example/"),
                Some("https://env.example".into()),
                Some(&env_file)
            ),
            ("https://dev.example".into(), "--site")
        );
        assert_eq!(
            resolve(None, Some("https://env.example".into()), Some(&env_file)),
            ("https://env.example".into(), "NECROPSY_BASE_URL")
        );
        assert_eq!(
            resolve(None, None, Some(&env_file)),
            ("http://localhost:8080".into(), ".env")
        );
        assert_eq!(
            resolve(None, Some("  ".into()), Some(&dir.join("missing"))),
            (DEFAULT_SITE.into(), "the default")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn loopback_sites_are_never_proxied() {
        for u in [
            "http://127.0.0.1:8080/x",
            "http://localhost:8080",
            "http://[::1]:80/",
            "http://dev.localhost/",
        ] {
            assert!(is_loopback(u), "{u}");
        }
        for u in [
            "https://necropsy.dev/api",
            "http://10.0.0.1/",
            "https://127.0.0.1.example/",
        ] {
            assert!(!is_loopback(u), "{u}");
        }
    }

    #[test]
    fn dot_env_parsing() {
        let t = "A=1\n#NECROPSY_BASE_URL=https://commented\nNECROPSY_BASE_URL=\"https://x\"\nNECROPSY_BASE_URL=\n";
        assert_eq!(
            env_file_value(t, "NECROPSY_BASE_URL").as_deref(),
            Some("https://x")
        );
        assert_eq!(env_file_value(t, "A").as_deref(), Some("1"));
        assert_eq!(env_file_value(t, "B"), None);
    }
}
