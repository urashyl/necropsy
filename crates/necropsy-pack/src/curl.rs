// SPDX-License-Identifier: AGPL-3.0-or-later
//! The curl subset of the workspace's request console, for Learn checkpoints
//! that ask for a curl command. It matches `static/toolkit/curl.js` in the web
//! crate: a URL, `-X`, `-H`, `-d`/`--data`/`--data-raw`, `-i`, `-v`.

/// A parsed command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Curl {
    pub method: String,
    pub host: String,
    pub path: String,
    /// `name=value` pairs of the query, in order, as typed.
    pub query: Vec<String>,
    /// Headers as given, names as typed.
    pub headers: Vec<(String, String)>,
    /// The body: every `--data` joined with `&`.
    pub data: Option<String>,
}

/// Splits a command line like a POSIX shell, for the quoting a curl command uses.
pub fn tokenize(line: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut has = false;
    let mut it = line.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\'' => {
                has = true;
                loop {
                    match it.next() {
                        Some('\'') => break,
                        Some(ch) => cur.push(ch),
                        None => return Err("A single quote is not closed.".into()),
                    }
                }
            }
            '"' => {
                has = true;
                loop {
                    match it.next() {
                        Some('"') => break,
                        Some('\\') if matches!(it.peek(), Some('"' | '\\' | '$' | '`' | '\n')) => {
                            let n = it.next().unwrap_or('\\');
                            if n != '\n' {
                                cur.push(n);
                            }
                        }
                        Some(ch) => cur.push(ch),
                        None => return Err("A double quote is not closed.".into()),
                    }
                }
            }
            '\\' => match it.next() {
                Some('\n') => {}
                Some('\r') if it.peek() == Some(&'\n') => {
                    it.next();
                }
                Some(ch) => {
                    cur.push(ch);
                    has = true;
                }
                None => {}
            },
            c if c.is_whitespace() => {
                if has {
                    out.push(std::mem::take(&mut cur));
                }
                has = false;
            }
            c => {
                cur.push(c);
                has = true;
            }
        }
    }
    if has {
        out.push(cur);
    }
    Ok(out)
}

fn value(
    flag: &str,
    args: &[String],
    i: &mut usize,
    attached: Option<&str>,
) -> Result<String, String> {
    if let Some(a) = attached.filter(|a| !a.is_empty()) {
        return Ok(a.to_owned());
    }
    *i += 1;
    args.get(*i)
        .cloned()
        .ok_or_else(|| format!("{flag} needs a value after it."))
}

/// Parses a curl command of the supported subset.
pub fn parse(line: &str) -> Result<Curl, String> {
    let args = tokenize(line)?;
    if args.first().map(String::as_str) != Some("curl") {
        return Err("The command must start with curl.".into());
    }
    let mut method: Option<String> = None;
    let mut headers = Vec::new();
    let mut data: Vec<String> = Vec::new();
    let mut url: Option<String> = None;
    let mut i = 1;
    while i < args.len() {
        let a = args[i].clone();
        if let Some(long) = a.strip_prefix("--") {
            let (name, attached) = match long.split_once('=') {
                Some((n, v)) => (n, Some(v)),
                None => (long, None),
            };
            match name {
                "request" => method = Some(value(&a, &args, &mut i, attached)?),
                "header" => headers.push(value(&a, &args, &mut i, attached)?),
                "data" | "data-raw" | "data-ascii" => {
                    data.push(value(&a, &args, &mut i, attached)?)
                }
                "include" | "verbose" => {}
                "url" => url = Some(value(&a, &args, &mut i, attached)?),
                _ => return Err(format!("Unsupported flag: --{name}.")),
            }
        } else if a.starts_with('-') && a.len() > 1 {
            let flag = &a[..2];
            let rest = Some(&a[2..]);
            match flag {
                "-X" => method = Some(value(flag, &args, &mut i, rest)?),
                "-H" => headers.push(value(flag, &args, &mut i, rest)?),
                "-d" => data.push(value(flag, &args, &mut i, rest)?),
                "-i" | "-v" => {
                    if let Some(bad) = a[1..].chars().find(|c| !matches!(c, 'i' | 'v')) {
                        return Err(format!("Unsupported flag: -{bad}."));
                    }
                }
                _ => return Err(format!("Unsupported flag: {flag}.")),
            }
        } else if url.is_some() {
            return Err("Only one URL per command.".into());
        } else {
            url = Some(a);
        }
        i += 1;
    }
    let url = url.ok_or("The command has no URL.")?;
    let lower = url.to_ascii_lowercase();
    let rest = ["http://", "https://"]
        .iter()
        .find_map(|s| lower.starts_with(s).then(|| &url[s.len()..]))
        .ok_or_else(|| format!("{url} is not an http:// or https:// URL."))?;
    let rest = rest.split('#').next().unwrap_or("");
    let split = rest.find(['/', '?']).unwrap_or(rest.len());
    let host = rest[..split].to_ascii_lowercase();
    if host.is_empty() {
        return Err(format!("{url} has no host."));
    }
    let after = &rest[split..];
    let (path, query) = match after.split_once('?') {
        Some((p, q)) => (p, q),
        None => (after, ""),
    };
    let headers = headers
        .iter()
        .map(|h| {
            let (n, v) = h
                .split_once(':')
                .ok_or_else(|| format!("-H '{h}' is not a header: write it as 'Name: value'."))?;
            Ok((n.trim().to_owned(), v.trim().to_owned()))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let method = method
        .unwrap_or_else(|| if data.is_empty() { "GET" } else { "POST" }.into())
        .to_ascii_uppercase();
    Ok(Curl {
        method,
        host,
        path: if path.is_empty() {
            "/".into()
        } else {
            path.to_owned()
        },
        query: query
            .split('&')
            .filter(|q| !q.is_empty())
            .map(str::to_owned)
            .collect(),
        headers,
        data: (!data.is_empty()).then(|| data.join("&")),
    })
}

impl Curl {
    /// The value of a header, by name in any case.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .rev()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// Whether this command sends the request `want` describes: the same method,
    /// host, path and body, the same query parameters in any order, and at least
    /// `want`'s headers with the same values. Extra headers are allowed.
    pub fn sends(&self, want: &Curl) -> bool {
        let mut q1 = self.query.clone();
        let mut q2 = want.query.clone();
        q1.sort();
        q2.sort();
        self.method == want.method
            && self.host == want.host
            && self.path == want.path
            && q1 == q2
            && self.data == want.data
            && want
                .headers
                .iter()
                .all(|(n, v)| self.header(n) == Some(v.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_like_the_console() {
        let c = parse(
            r#"curl -i -X post -H 'User-Agent: Pebble/2.4 (Win64)' -H "X-Id: 7" --data 'a=1' -d b=2 'https://h.test/p/q?x=1&y=2'"#,
        )
        .unwrap();
        assert_eq!(c.method, "POST");
        assert_eq!(c.host, "h.test");
        assert_eq!(c.path, "/p/q");
        assert_eq!(c.query, ["x=1", "y=2"]);
        assert_eq!(c.header("user-agent"), Some("Pebble/2.4 (Win64)"));
        assert_eq!(c.data.as_deref(), Some("a=1&b=2"));
        assert_eq!(parse("curl https://h.test").unwrap().path, "/");
        assert_eq!(parse("curl -d x https://h.test").unwrap().method, "POST");
        assert_eq!(parse("curl -XPUT https://h.test").unwrap().method, "PUT");
        assert_eq!(
            tokenize("curl \\\n -H 'a b' \"c\\\"d\"").unwrap(),
            ["curl", "-H", "a b", "c\"d"]
        );
    }

    #[test]
    fn refuses_what_the_console_refuses() {
        for (cmd, want) in [
            ("wget x", "must start with curl"),
            ("curl -A x https://h.test", "Unsupported flag: -A"),
            (
                "curl --cookie a=b https://h.test",
                "Unsupported flag: --cookie",
            ),
            ("curl -ik https://h.test", "Unsupported flag: -k"),
            ("curl -H nocolon https://h.test", "is not a header"),
            ("curl ftp://h.test", "not an http"),
            ("curl -i", "no URL"),
            ("curl 'open", "not closed"),
        ] {
            let e = parse(cmd).unwrap_err();
            assert!(e.contains(want), "{cmd}: {e}");
        }
    }

    #[test]
    fn sends_compares_the_request_not_the_spelling() {
        let want = parse("curl -X POST -H 'X-Operator: nightjar' --data 'user=op' 'https://h.test/login?a=1&b=2'").unwrap();
        for ok in [
            "curl -H 'x-operator: nightjar' -d user=op 'https://H.test/login?b=2&a=1'",
            "curl -i -v --data=user=op --header='X-Operator: nightjar' -H 'Accept: */*' 'https://h.test/login?a=1&b=2'",
        ] {
            assert!(parse(ok).unwrap().sends(&want), "{ok}");
        }
        for bad in [
            "curl -H 'X-Operator: nightjar' 'https://h.test/login?a=1&b=2'",
            "curl -H 'X-Operator: nightjar' -d user=op 'https://h.test/login?a=1'",
            "curl -H 'X-Operator: other' -d user=op 'https://h.test/login?a=1&b=2'",
            "curl -X PUT -H 'X-Operator: nightjar' -d user=op 'https://h.test/login?a=1&b=2'",
            "curl -H 'X-Operator: nightjar' -d user=op 'https://h.test/logout?a=1&b=2'",
        ] {
            assert!(!parse(bad).unwrap().sends(&want), "{bad}");
        }
    }
}
