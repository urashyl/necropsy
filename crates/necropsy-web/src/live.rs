// SPDX-License-Identifier: AGPL-3.0-or-later
//! Live puzzles: a server of its own for each player.
//!
//! A player signed in to the main site presses "Spin up" on a live puzzle and
//! gets an instance, reachable at `https://<label>.<NECROPSY_LIVE_DOMAIN>`.
//! Your reverse proxy sends `*.<domain>` to this same process; requests whose
//! `Host` falls under the domain are answered here from the pack's scenario
//! (see `necropsy_pack::scenario`) and never reach the main site's pages,
//! cookies or CSRF logic. See `docs/live.md`.
//!
//! Nothing a scenario describes is executed: a response is data from the pack.
//! The flag and any `{{d:name}}` values come from a per-instance random seed,
//! so they differ for every player and a flag cannot be passed around.

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::{header, HeaderName, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine;
use necropsy_pack::scenario::{Respond, Route, Scenario, When};
use necropsy_pack::Pack;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::auth::{new_token, random_string, MaybeUser};
use crate::db::{now, Instance};
use crate::state::AppState;

/// Settings for live puzzles. Leave `NECROPSY_LIVE_DOMAIN` unset to turn them off.
pub struct LiveConfig {
    /// Parent domain of instances, as seen in the `Host` header, port included
    /// when there is one (`necropsy.urashyl.dev`, or `necropsy.localhost:8080`).
    pub domain: String,
    /// Optional header carrying the original host, for a proxy that rewrites
    /// `Host`. Believed only from `NECROPSY_TRUSTED_PROXIES` (any peer if none).
    pub host_header: Option<HeaderName>,
    /// The spawner, for container-backed puzzles. Without it those puzzles
    /// are hidden.
    pub spawner: Option<Spawner>,
}

/// Where the spawner listens, and where it puts each instance's socket.
pub struct Spawner {
    pub socket: std::path::PathBuf,
    pub range_dir: std::path::PathBuf,
}

/// An instance of a container puzzle is kept alive while it is used, but never
/// longer than this from when it started.
const MAX_LIFETIME_SECS: i64 = 8 * 3600;
/// Most the container's own answer to a request may be.
const MAX_PROXY_RESPONSE: usize = 8 * 1024 * 1024;

/// Largest request body an instance reads.
const MAX_BODY: usize = 1024 * 1024;
/// Requests an instance answers per rate window.
const RATE_MAX: u32 = 100;
const RATE_WINDOW: Duration = Duration::from_secs(10);
/// Log entries kept per instance.
const LOG_KEEP: usize = 100;
/// Running instances one account may have at once.
const MAX_ACTIVE_PER_USER: i64 = 3;

#[derive(Clone, Serialize)]
pub struct LogEntry {
    pub t: i64,
    pub method: String,
    pub path: String,
    pub status: u16,
    pub ua: String,
}

/// In-memory request logs and rate counters, per instance label.
#[derive(Default)]
pub struct LiveHub {
    logs: Mutex<HashMap<String, VecDeque<LogEntry>>>,
    rate: Mutex<HashMap<String, (Instant, u32)>>,
}

impl LiveHub {
    fn push(&self, label: &str, e: LogEntry) {
        let mut logs = self.logs.lock().unwrap_or_else(|p| p.into_inner());
        let q = logs.entry(label.to_owned()).or_default();
        if q.len() >= LOG_KEEP {
            q.pop_front();
        }
        q.push_back(e);
    }

    pub fn recent(&self, label: &str) -> Vec<LogEntry> {
        let logs = self.logs.lock().unwrap_or_else(|p| p.into_inner());
        logs.get(label)
            .map(|q| q.iter().cloned().collect())
            .unwrap_or_default()
    }

    fn allow(&self, label: &str) -> bool {
        let mut rate = self.rate.lock().unwrap_or_else(|p| p.into_inner());
        let e = rate.entry(label.to_owned()).or_insert((Instant::now(), 0));
        if e.0.elapsed() > RATE_WINDOW {
            *e = (Instant::now(), 0);
        }
        e.1 += 1;
        e.1 <= RATE_MAX
    }

    /// Drops counters and logs of instances that no longer exist.
    pub fn prune(&self, keep: impl Fn(&str) -> bool) {
        self.logs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|k, _| keep(k));
        self.rate
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|_, (t, _)| t.elapsed() <= RATE_WINDOW);
    }
}

// ---------------------------------------------------------------- derivation

fn derive(seed: &str, tag: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"necropsy:live:v1:");
    h.update(tag.as_bytes());
    h.update(b":");
    h.update(seed.as_bytes());
    h.finalize().iter().fold(String::new(), |mut s, b| {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// The flag of an instance: `prefix{20 hex digits}`.
pub fn flag_of(scenario: &Scenario, pack_id: &str, seed: &str) -> String {
    format!(
        "{}{{{}}}",
        scenario.flag_prefix,
        &derive(seed, &format!("flag:{pack_id}"))[..20]
    )
}

struct Ctx<'a> {
    flag: String,
    seed: &'a str,
    label: &'a str,
    host: &'a str,
}

/// Fills in `{{flag}}`, `{{instance}}`, `{{host}}` and `{{d:name}}`.
fn render(t: &str, ctx: &Ctx) -> String {
    let mut out = String::with_capacity(t.len());
    let mut rest = t;
    while let Some(i) = rest.find("{{") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        let Some(j) = after.find("}}") else {
            out.push_str(&rest[i..]);
            return out;
        };
        let name = &after[..j];
        match name {
            "flag" => out.push_str(&ctx.flag),
            "instance" => out.push_str(ctx.label),
            "host" => out.push_str(ctx.host),
            _ => match name.strip_prefix("d:") {
                Some(d) => out.push_str(&derive(ctx.seed, &format!("d:{d}"))[..12]),
                None => {
                    out.push_str("{{");
                    out.push_str(name);
                    out.push_str("}}");
                }
            },
        }
        rest = &after[j + 2..];
    }
    out.push_str(rest);
    out
}

// ------------------------------------------------------------------ requests

fn percent_decode(s: &str, plus_is_space: bool) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(v) => {
                        out.push(v);
                        i += 3;
                    }
                    None => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' if plus_is_space => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn pairs(s: &str) -> Vec<(String, String)> {
    s.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (percent_decode(k, true), percent_decode(v, true))
        })
        .collect()
}

struct Incoming {
    method: Method,
    path: String,
    headers: axum::http::HeaderMap,
    query: Vec<(String, String)>,
    form: Vec<(String, String)>,
    body: String,
}

impl Incoming {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }

    fn cookie(&self, name: &str) -> Option<String> {
        crate::auth::cookie_value(&self.headers, name)
    }

    fn basic_auth(&self) -> Option<String> {
        let v = self.header("authorization")?;
        let b64 = v
            .strip_prefix("Basic ")
            .or_else(|| v.strip_prefix("basic "))?;
        let raw = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .ok()?;
        String::from_utf8(raw).ok()
    }
}

/// Whether a condition value accepts what the request had (`None` = absent).
fn accepts(pattern: &str, actual: Option<&str>, ctx: &Ctx) -> bool {
    let p = render(pattern, ctx);
    if p == "*" {
        return actual.is_some();
    }
    let Some(actual) = actual else {
        return false;
    };
    match p.strip_prefix("re:") {
        Some(re) => regex::RegexBuilder::new(re)
            .size_limit(1 << 20)
            .build()
            .is_ok_and(|re| re.is_match(actual)),
        None => p == actual,
    }
}

fn first<'a>(v: &'a [(String, String)], k: &str) -> Option<&'a str> {
    v.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
}

fn when_holds(w: &When, req: &Incoming, ctx: &Ctx) -> bool {
    w.header
        .iter()
        .all(|(k, v)| accepts(v, req.header(&k.to_ascii_lowercase()), ctx))
        && w.query
            .iter()
            .all(|(k, v)| accepts(v, first(&req.query, k), ctx))
        && w.cookie
            .iter()
            .all(|(k, v)| accepts(v, req.cookie(k).as_deref(), ctx))
        && w.form
            .iter()
            .all(|(k, v)| accepts(v, first(&req.form, k), ctx))
        && w.body
            .as_ref()
            .is_none_or(|b| accepts(b, Some(&req.body), ctx))
        && w.basic_auth
            .as_ref()
            .is_none_or(|b| req.basic_auth().as_deref() == Some(render(b, ctx).as_str()))
}

fn route_matches(r: &Route, req: &Incoming, flags: &[String], ctx: &Ctx) -> bool {
    if let Some(m) = &r.method {
        let ok = req.method.as_str() == m || (m == "GET" && req.method == Method::HEAD);
        if !ok {
            return false;
        }
    }
    let path = render(&r.path, ctx);
    let path_ok = match path.strip_suffix('*') {
        Some(prefix) => req.path.starts_with(prefix),
        None => req.path == path,
    };
    path_ok && r.needs.iter().all(|n| flags.contains(n)) && when_holds(&r.when, req, ctx)
}

fn respond(r: &Respond, pack: &Pack, ctx: &Ctx) -> Response {
    let mut builder = Response::builder()
        .status(StatusCode::from_u16(r.status).unwrap_or(StatusCode::OK))
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::REFERRER_POLICY, "no-referrer")
        .header(header::X_FRAME_OPTIONS, "DENY")
        .header(header::CACHE_CONTROL, "no-store");
    let mut has_type = false;
    for (k, v) in &r.headers {
        if let (Ok(name), Ok(val)) = (
            HeaderName::try_from(k.as_str()),
            HeaderValue::from_str(&render(v, ctx)),
        ) {
            has_type |= name == header::CONTENT_TYPE;
            builder = builder.header(name, val);
        }
    }
    if !has_type {
        builder = builder.header(
            header::CONTENT_TYPE,
            r.content_type
                .as_deref()
                .unwrap_or("text/plain; charset=utf-8"),
        );
    }
    let body: Vec<u8> = match (&r.body, &r.file) {
        (Some(b), _) => render(b, ctx).into_bytes(),
        (None, Some(f)) => pack
            .live_file(f)
            .map(|f| f.data.clone())
            .unwrap_or_default(),
        (None, None) => Vec::new(),
    };
    builder
        .body(Body::from(body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn plain(status: StatusCode, text: &'static str) -> Response {
    (
        status,
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        text,
    )
        .into_response()
}

// ------------------------------------------------------------------ dispatch

/// The instance label if this request is for the live domain. `Some("")`
/// means "under the domain but not a valid instance name".
pub fn live_label(state: &AppState, req: &Request) -> Option<String> {
    let cfg = state.live.as_ref()?;
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip());
    let trusted = state.trusted_proxies.is_empty()
        || peer.is_some_and(|p| state.trusted_proxies.iter().any(|n| n.contains(p)));
    let host = cfg
        .host_header
        .as_ref()
        .filter(|_| trusted)
        .and_then(|h| req.headers().get(h))
        .or_else(|| req.headers().get(header::HOST))
        .and_then(|v| v.to_str().ok())?
        .to_ascii_lowercase();
    let host = host.trim_end_matches('.');
    let prefix = host.strip_suffix(&cfg.domain)?.strip_suffix('.')?;
    let valid = !prefix.is_empty()
        && prefix.len() <= 40
        && prefix
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    Some(if valid {
        prefix.to_owned()
    } else {
        String::new()
    })
}

pub async fn handle(state: Arc<AppState>, label: String, req: Request) -> Response {
    if label.is_empty() {
        return plain(StatusCode::NOT_FOUND, "No such instance.\n");
    }
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let (parts, body) = req.into_parts();

    let inst = match state.db.live_by_label(label.clone()).await {
        Ok(Some(i)) => i,
        Ok(None) => return plain(StatusCode::NOT_FOUND, "No such instance.\n"),
        Err(e) => {
            tracing::error!("live lookup failed: {e}");
            return plain(StatusCode::INTERNAL_SERVER_ERROR, "Server error.\n");
        }
    };
    if now() >= inst.expires_at {
        return plain(StatusCode::GONE, "This instance has expired.\n");
    }
    let Some(pack) = state.get(&inst.pack_id).map(|p| &p.pack) else {
        return plain(StatusCode::NOT_FOUND, "No such instance.\n");
    };
    let Some(scenario) = &pack.live else {
        return plain(StatusCode::NOT_FOUND, "No such instance.\n");
    };
    if !state.hub.allow(&label) {
        return plain(StatusCode::TOO_MANY_REQUESTS, "Slow down.\n");
    }
    let bytes = match axum::body::to_bytes(body, MAX_BODY).await {
        Ok(b) => b,
        Err(_) => return plain(StatusCode::PAYLOAD_TOO_LARGE, "Request body too large.\n"),
    };

    let info = req_info(&parts);
    if scenario.container.is_some() {
        let res = proxy(&state, &inst, scenario, &parts, bytes).await;
        log_request(&state, &label, &info, res.status());
        return res;
    }

    let body_text = String::from_utf8_lossy(&bytes).into_owned();
    let is_form = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/x-www-form-urlencoded"));
    let incoming = Incoming {
        path: percent_decode(parts.uri.path(), false),
        query: parts.uri.query().map(pairs).unwrap_or_default(),
        form: if is_form {
            pairs(&body_text)
        } else {
            Vec::new()
        },
        body: body_text,
        method: parts.method,
        headers: parts.headers,
    };
    let ctx = Ctx {
        flag: flag_of(scenario, &inst.pack_id, &inst.seed),
        seed: &inst.seed,
        label: &label,
        host: &host,
    };

    let hit = scenario
        .routes
        .iter()
        .find(|r| route_matches(r, &incoming, &inst.flags, &ctx));
    let res = match hit {
        Some(r) => {
            let mut flags = inst.flags.clone();
            for s in &r.sets {
                if !flags.contains(s) {
                    flags.push(s.clone());
                }
            }
            if flags.len() != inst.flags.len() {
                if let Err(e) = state.db.live_set_flags(label.clone(), flags).await {
                    tracing::error!("saving instance state failed: {e}");
                }
            }
            respond(&r.respond, pack, &ctx)
        }
        None => match &scenario.fallback {
            Some(f) => respond(f, pack, &ctx),
            None => plain(StatusCode::NOT_FOUND, "Not Found\n"),
        },
    };

    log_request(&state, &label, &info, res.status());
    res
}

struct ReqInfo {
    method: String,
    target: String,
    ua: String,
}

fn req_info(parts: &axum::http::request::Parts) -> ReqInfo {
    ReqInfo {
        method: parts.method.to_string(),
        target: parts
            .uri
            .path_and_query()
            .map(|p| p.as_str().chars().take(200).collect())
            .unwrap_or_default(),
        ua: parts
            .headers
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .chars()
            .take(120)
            .collect(),
    }
}

fn log_request(state: &AppState, label: &str, info: &ReqInfo, status: StatusCode) {
    state.hub.push(
        label,
        LogEntry {
            t: now(),
            method: info.method.clone(),
            path: info.target.clone(),
            status: status.as_u16(),
            ua: info.ua.clone(),
        },
    );
}

// ------------------------------------------------------------------ containers

/// Headers that belong to one connection and are never passed along.
fn hop_by_hop(name: &str) -> bool {
    matches!(
        name,
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "content-length"
    )
}

fn spawner_req(
    method: &str,
    uri: &str,
    json: Option<String>,
) -> hyper::Request<http_body_util::Full<axum::body::Bytes>> {
    hyper::Request::builder()
        .method(method)
        .uri(uri)
        .header(header::HOST, "spawner")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CONNECTION, "close")
        .body(http_body_util::Full::new(axum::body::Bytes::from(
            json.unwrap_or_default(),
        )))
        .expect("a fixed request is valid")
}

/// Calls the spawner. `Ok` only for a 2xx answer.
async fn spawner_call(
    sp: &Spawner,
    method: &str,
    uri: &str,
    json: Option<String>,
) -> Result<(), (StatusCode, String)> {
    let res = crate::unixhttp::send(&sp.socket, spawner_req(method, uri, json), 4096)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, e))?;
    if res.status().is_success() {
        Ok(())
    } else {
        let text = String::from_utf8_lossy(res.body()).into_owned();
        Err((res.status(), text))
    }
}

/// Starts the container for a new instance.
async fn start_container(
    state: &AppState,
    inst: &Instance,
    scenario: &Scenario,
    image: &str,
) -> Result<(), Response> {
    let Some(sp) = state.live.as_ref().and_then(|l| l.spawner.as_ref()) else {
        return Err(StatusCode::NOT_FOUND.into_response());
    };
    let body = serde_json::json!({
        "id": inst.label,
        "image": image,
        "ttl_secs": inst.expires_at - inst.created_at,
        "env": {
            "NECROPSY_FLAG": flag_of(scenario, &inst.pack_id, &inst.seed),
            "NECROPSY_INSTANCE": inst.label,
        },
    });
    match spawner_call(sp, "POST", "/v1/instances", Some(body.to_string())).await {
        Ok(()) => Ok(()),
        Err((status, why)) => {
            tracing::error!("spawner refused {}: {status} {why}", inst.label);
            let msg = if status == StatusCode::TOO_MANY_REQUESTS {
                "The range is full right now. Try again in a few minutes."
            } else {
                "Could not start the instance. Try again."
            };
            Err((StatusCode::SERVICE_UNAVAILABLE, msg).into_response())
        }
    }
}

/// Stops a container and forgets it. Best effort: the spawner reaps by time
/// anyway.
async fn stop_container(state: &AppState, label: &str) {
    if let Some(sp) = state.live.as_ref().and_then(|l| l.spawner.as_ref()) {
        if let Err((status, why)) =
            spawner_call(sp, "DELETE", &format!("/v1/instances/{label}"), None).await
        {
            tracing::warn!("stopping {label}: {status} {why}");
        }
    }
}

/// Keeps a container that is being used alive: when less than half of its
/// time is left, give it a full term again, up to a cap from when it started.
fn keep_alive(state: &Arc<AppState>, inst: &Instance, ttl: i64) {
    let t = now();
    if inst.expires_at - t > ttl / 2 {
        return;
    }
    let new_expiry = (t + ttl).min(inst.created_at + MAX_LIFETIME_SECS);
    if new_expiry <= inst.expires_at + 30 {
        return;
    }
    let (state, label) = (state.clone(), inst.label.clone());
    tokio::spawn(async move {
        if let Err(e) = state.db.live_set_expiry(label.clone(), new_expiry).await {
            tracing::error!("extending {label} failed: {e}");
            return;
        }
        if let Some(sp) = state.live.as_ref().and_then(|l| l.spawner.as_ref()) {
            let body = format!(r#"{{"ttl_secs":{}}}"#, new_expiry - now());
            let _ = spawner_call(
                sp,
                "POST",
                &format!("/v1/instances/{label}/touch"),
                Some(body),
            )
            .await;
        }
    });
}

/// Passes a request to the instance's container, over its unix socket, and
/// returns what it answers.
async fn proxy(
    state: &Arc<AppState>,
    inst: &Instance,
    scenario: &Scenario,
    parts: &axum::http::request::Parts,
    body: axum::body::Bytes,
) -> Response {
    let Some(sp) = state.live.as_ref().and_then(|l| l.spawner.as_ref()) else {
        return plain(StatusCode::NOT_FOUND, "No such instance.\n");
    };
    keep_alive(state, inst, i64::from(scenario.ttl_minutes) * 60);

    let mut b = hyper::Request::builder().method(parts.method.clone()).uri(
        parts
            .uri
            .path_and_query()
            .map(|p| p.as_str())
            .unwrap_or("/"),
    );
    for (k, v) in &parts.headers {
        if !hop_by_hop(k.as_str()) {
            b = b.header(k, v);
        }
    }
    let req = match b
        .header(header::CONNECTION, "close")
        .header(header::CONTENT_LENGTH, body.len())
        .body(http_body_util::Full::new(body))
    {
        Ok(r) => r,
        Err(_) => return plain(StatusCode::BAD_REQUEST, "Bad request.\n"),
    };
    let socket = sp.range_dir.join(&inst.label).join("app.sock");
    let res = match crate::unixhttp::send(&socket, req, MAX_PROXY_RESPONSE).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("instance {} did not answer: {e}", inst.label);
            return plain(StatusCode::BAD_GATEWAY, "The instance is not answering.\n");
        }
    };
    let (rp, rbody) = res.into_parts();
    let mut out = Response::builder().status(rp.status);
    for (k, v) in &rp.headers {
        if !hop_by_hop(k.as_str()) {
            out = out.header(k, v);
        }
    }
    // Defaults the container's own headers can override.
    for (k, v) in [
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::REFERRER_POLICY, "no-referrer"),
        (header::X_FRAME_OPTIONS, "DENY"),
        (header::CACHE_CONTROL, "no-store"),
    ] {
        if !rp.headers.contains_key(&k) {
            out = out.header(k, v);
        }
    }
    out.body(Body::from(rbody))
        .unwrap_or_else(|_| plain(StatusCode::BAD_GATEWAY, "Bad answer from the instance.\n"))
}

// ------------------------------------------------------------------ main-site API

#[derive(Serialize)]
struct InstanceView {
    active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    seconds_left: Option<i64>,
    log: Vec<LogEntry>,
}

fn view(state: &AppState, inst: Option<&Instance>) -> InstanceView {
    match (inst, &state.live) {
        (Some(i), Some(cfg)) => {
            let scheme = state.base_url.split("://").next().unwrap_or("https");
            InstanceView {
                active: true,
                url: Some(format!("{scheme}://{}.{}", i.label, cfg.domain)),
                seconds_left: Some((i.expires_at - now()).max(0)),
                log: state.hub.recent(&i.label),
            }
        }
        _ => InstanceView {
            active: false,
            url: None,
            seconds_left: None,
            log: Vec::new(),
        },
    }
}

/// Resolves the pack and the signed-in user, or the error response.
fn live_pack<'a>(
    state: &'a AppState,
    viewer: &Option<crate::db::User>,
    id: &str,
) -> Result<(&'a Pack, i64), (StatusCode, &'static str)> {
    let Some(p) = state
        .get(id)
        .filter(|p| p.pack.is_live() && state.live.is_some())
    else {
        return Err((StatusCode::NOT_FOUND, ""));
    };
    match viewer {
        Some(u) => Ok((&p.pack, u.id)),
        None => Err((StatusCode::UNAUTHORIZED, "Sign in to spin up an instance.")),
    }
}

pub async fn instance_get(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    Path(id): Path<String>,
) -> Response {
    let (_, uid) = match live_pack(&state, &viewer, &id) {
        Ok(v) => v,
        Err(r) => return r.into_response(),
    };
    match state.db.live_active(uid, id).await {
        Ok(inst) => Json(view(&state, inst.as_ref())).into_response(),
        Err(e) => {
            tracing::error!("live lookup failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Starts an instance, or returns the one already running.
pub async fn instance_post(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    crate::auth::ClientIp(ip): crate::auth::ClientIp,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let (pack, uid) = match live_pack(&state, &viewer, &id) {
        Ok(v) => v,
        Err(r) => return r.into_response(),
    };
    let Some(scenario) = &pack.live else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let server_error = |e: rusqlite::Error| {
        tracing::error!("live instance failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR.into_response()
    };
    match state.db.live_active(uid, id.clone()).await {
        Ok(Some(inst)) => return Json(view(&state, Some(&inst))).into_response(),
        Ok(None) => {}
        Err(e) => return server_error(e),
    }
    // Only a new instance costs anything, so only that needs the captcha.
    let token = headers
        .get("x-turnstile-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !state.captcha.verify(token, &ip).await {
        return (
            StatusCode::FORBIDDEN,
            "The captcha check failed. Try again.",
        )
            .into_response();
    }
    match state.db.live_active_count(uid).await {
        Ok(n) if n >= MAX_ACTIVE_PER_USER => {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                "You have too many running instances. Stop one first.",
            )
                .into_response()
        }
        Ok(_) => {}
        Err(e) => return server_error(e),
    }
    let t = now();
    let inst = Instance {
        label: random_string(b"abcdefghjkmnpqrstuvwxyz23456789", 10),
        user_id: uid,
        pack_id: id,
        seed: new_token(),
        created_at: t,
        expires_at: t + i64::from(scenario.ttl_minutes) * 60,
        flags: Vec::new(),
    };
    if let Err(e) = state.db.live_create(inst.clone()).await {
        return server_error(e);
    }
    if let Some(image) = pack.container() {
        if let Err(res) = start_container(&state, &inst, scenario, image).await {
            let _ = state.db.live_delete(inst.label.clone()).await;
            return res;
        }
    }
    Json(view(&state, Some(&inst))).into_response()
}

pub async fn instance_stop(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    Path(id): Path<String>,
) -> Response {
    let (pack, uid) = match live_pack(&state, &viewer, &id) {
        Ok(v) => v,
        Err(r) => return r.into_response(),
    };
    if pack.container().is_some() {
        if let Ok(Some(inst)) = state.db.live_active(uid, id.clone()).await {
            stop_container(&state, &inst.label).await;
        }
    }
    match state.db.live_stop(uid, id).await {
        Ok(()) => Json(view(&state, None)).into_response(),
        Err(e) => {
            tracing::error!("live stop failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Whether `guess` is the flag of any instance the user has had of this pack.
pub async fn flag_matches(state: &AppState, user_id: i64, pack: &Pack, guess: &str) -> bool {
    let Some(scenario) = &pack.live else {
        return false;
    };
    let want = necropsy_pack::normalize_answer(guess);
    match state.db.live_of(user_id, pack.meta.id.clone()).await {
        Ok(list) => list.iter().any(|i| {
            necropsy_pack::normalize_answer(&flag_of(scenario, &pack.meta.id, &i.seed)) == want
        }),
        Err(e) => {
            tracing::error!("live flag check failed: {e}");
            false
        }
    }
}
