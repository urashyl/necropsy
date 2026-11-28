// SPDX-License-Identifier: AGPL-3.0-or-later
//! necropsy web front end.
//!
//! Server-rendered pages over a directory of packs, plus accounts (see
//! `docs/web.md` and `docs/accounts.md`). Visitors can solve puzzles without
//! an account; their progress then lives in the browser's localStorage. A
//! signed-in user's solves are also stored on the server. The server decides
//! whether a submitted answer is right, so the explainer is not sent until it
//! is.

mod accounts;
mod admin;
mod auth;
mod captcha;
mod cards;
mod community;
mod db;
mod generated;
mod ipnet;
mod leaderboard;
mod learn;
mod live;
mod mail;
mod markdown;
mod mfa;
mod moderation;
mod pages;
mod progress;
mod sealed;
use necropsy_pack::seal;
mod state;
mod toolkit;
mod workspace;
mod wsconsole;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tower_http::compression::CompressionLayer;

use crate::auth::{ClientIp, MaybeUser, AUTH_WINDOW};
use crate::db::Db;
use crate::mail::Mailer;
use crate::state::{AppState, Config};

#[tokio::main]
async fn main() {
    // `necropsy-web healthcheck` is what the container's HEALTHCHECK runs, so
    // the image needs no curl or shell. It exits 0 only if /healthz answers.
    // `necropsy-web generate` runs one pack generator for the server, in a child
    // process with nothing in its environment (see generated.rs).
    if std::env::args().nth(1).as_deref() == Some("generate") {
        std::process::exit(match generated::child_main() {
            c if c == std::process::ExitCode::SUCCESS => 0,
            _ => 1,
        });
    }
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        let addr = env_or("NECROPSY_ADDR", "127.0.0.1:8080");
        let ok = addr.parse::<SocketAddr>().map(healthcheck).unwrap_or(false);
        std::process::exit(if ok { 0 } else { 1 });
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "necropsy_web=info".into()),
        )
        .init();

    let addr: SocketAddr = env_or("NECROPSY_ADDR", "127.0.0.1:8080")
        .parse()
        .expect("NECROPSY_ADDR must be host:port");
    let packs_dir = PathBuf::from(env_or("NECROPSY_PACKS", "packs"));
    let db_path = env_or("NECROPSY_DB", "necropsy.db");
    let config = Config {
        base_url: env_or("NECROPSY_BASE_URL", "https://necropsy.urashyl.dev"),
        client_ip_header: client_ip_header_from_env().unwrap_or_else(|e| fail(&e)),
        trusted_proxies: trusted_proxies_from_env().unwrap_or_else(|e| fail(&e)),
        live: live_from_env().unwrap_or_else(|e| fail(&e)),
        captcha: captcha_from_env().unwrap_or_else(|e| fail(&e)),
        log_requests: std::env::var("NECROPSY_LOG_REQUESTS").is_ok_and(|v| v == "1"),
    };
    if config.captcha.site_key().is_none() {
        tracing::warn!(
            "no captcha: set NECROPSY_TURNSTILE_SITE_KEY and NECROPSY_TURNSTILE_SECRET so scripts \
             cannot mass-register accounts or start live instances"
        );
    }

    let packs = match necropsy_pack::load_all(&packs_dir) {
        Ok(p) => p,
        Err(e) => fail(&format!(
            "cannot load packs from {}: {e}",
            packs_dir.display()
        )),
    };
    tracing::info!("loaded {} packs from {}", packs.len(), packs_dir.display());

    let db = Db::open(&db_path)
        .unwrap_or_else(|e| fail(&format!("cannot open database {db_path}: {e}")));
    let mailer = Mailer::from_env().unwrap_or_else(|e| fail(&e));
    tracing::info!("email: {}", mailer.describe());

    // First run: there is no default admin. One is generated here and its
    // credentials are printed once, below, and never stored in plaintext.
    match auth::bootstrap_admin(&db).await {
        Ok(Some((username, password))) => tracing::warn!(
            "first run: created the admin account. These credentials are shown once and cannot be recovered.\n\
             \n    username: {username}\n    password: {password}\n\n\
             Log in at /login, then change the password at /account."
        ),
        Ok(None) => {}
        Err(e) => fail(&e),
    }

    match (&config.client_ip_header, config.trusted_proxies.is_empty()) {
        (Some(h), true) => tracing::warn!(
            "{h} is believed from any client. Set NECROPSY_TRUSTED_PROXIES to your proxy's address \
             so only it can supply the client address."
        ),
        (None, false) => tracing::warn!(
            "NECROPSY_TRUSTED_PROXIES has no effect without NECROPSY_CLIENT_IP_HEADER"
        ),
        _ => {}
    }

    let learn_dir = PathBuf::from(env_or("NECROPSY_LEARN", "learn"));
    let modules = match necropsy_pack::learn::load_all(&learn_dir, &packs) {
        Ok(m) => m,
        Err(e) => fail(&format!(
            "cannot load the Learn modules from {}: {e}",
            learn_dir.display()
        )),
    };
    tracing::info!(
        "loaded {} Learn modules from {}",
        modules.len(),
        learn_dir.display()
    );
    // The server secret keys every generated pack's seeds and every live flag.
    // By default it sits next to the database; see docs/generators.md.
    let secret_path = std::env::var("NECROPSY_SERVER_SECRET_FILE")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(&db_path)
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(std::path::Path::new("."))
                .join("server-secret")
        });
    let secret = generated::load_secret(&secret_path).unwrap_or_else(|e| fail(&e));
    let exe = std::env::current_exe()
        .unwrap_or_else(|e| fail(&format!("cannot find this program's own path: {e}")));
    let generator = generated::Generator::new(secret, generated::Runner::Process(exe));
    // Sealed packs (docs/sealing.md): opened with the master key, checked, and
    // served like any other daily. The master key is made, next to the server
    // secret, if there is none.
    let master_default = secret_path.with_file_name("master-key");
    let (master, made) = sealed::load_master(
        std::env::var("NECROPSY_MASTER_KEY_FILE").ok(),
        std::env::var("NECROPSY_MASTER_KEY").ok(),
        &master_default,
    )
    .unwrap_or_else(|e| fail(&e));
    let sealed_dir = PathBuf::from(env_or("NECROPSY_SEALED", "packs-sealed"));
    if made {
        tracing::warn!(
            "created a new master key in {}: back it up, since no pack sealed to it opens without it",
            master_default.display()
        );
    }
    let wrap = master.current_wrap_key();
    match seal::committed_wrap_keys(&sealed_dir)
        .ok()
        .and_then(|mut keys| keys.remove(&wrap.version))
    {
        Some(c) if c.public != wrap.public => tracing::warn!(
            "wrap key v{} ({}) is not the committed one ({}): packs sealed for this repository will not open here",
            wrap.version,
            wrap.fingerprint_hex(),
            c.fingerprint_hex()
        ),
        Some(_) => {}
        None => tracing::warn!(
            "wrap key v{} ({}) is not committed: run `pack-tool fetch-wrap-key` against this site and commit it",
            wrap.version,
            wrap.fingerprint_hex()
        ),
    }
    let (sealed_packs, sealed_state) =
        sealed::load(&sealed_dir, Some(&master), &packs, &mut |wasm, seed| {
            generator.run_blocking(wasm, seed)
        });
    tracing::info!(
        "sealed packs: {} opened from {}, wrap key v{}",
        sealed_packs.len(),
        sealed_dir.display(),
        master.current_version()
    );
    for s in sealed_state.status.iter().filter(|s| !s.ok) {
        tracing::warn!("sealed pack {}: {}", s.id, s.outcome);
    }
    let mut packs = packs;
    packs.extend(sealed_packs);
    let mut state = AppState::new(packs, config, db, mailer);
    state.generator = generator;
    state.top_n = match std::env::var("NECROPSY_RANKED_TOP_N") {
        Ok(v) if !v.trim().is_empty() => match v.trim().parse::<usize>() {
            Ok(n) if (1..=1000).contains(&n) => n,
            _ => fail("NECROPSY_RANKED_TOP_N must be a number from 1 to 1000"),
        },
        _ => moderation::DEFAULT_TOP_N,
    };
    state.sealed = sealed_state;
    state.set_learn(modules);
    state.community = community::from_env().unwrap_or_else(|e| fail(&e));

    let toolkit_dir = PathBuf::from(env_or("NECROPSY_TOOLKIT", "toolkit/dist"));
    state.toolkit = toolkit::Toolkit::load(&toolkit_dir);
    if state.toolkit.complete() {
        tracing::info!(
            "toolkit: {} files from {}",
            state.toolkit.file_count(),
            toolkit_dir.display()
        );
    } else {
        tracing::warn!(
            "toolkit: not every tool is in {} (run toolkit/fetch.sh); the workspace says which are missing",
            toolkit_dir.display()
        );
    }
    let state = Arc::new(state);
    spawn_housekeeping(state.clone());
    state.community.load(&state.db).await;
    if state.community.forum.is_some() {
        let state = state.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(community::INTERVAL);
            loop {
                tick.tick().await;
                community::post_due_threads(&state).await;
            }
        });
    }
    // Say at startup whether the range can be reached from here, which is where a
    // misconfigured network or firewall shows up first.
    if state.live.as_ref().is_some_and(|l| l.range.is_some()) {
        let state = state.clone();
        tokio::spawn(async move {
            let Some(range) = state.live.as_ref().and_then(|l| l.range.as_ref()) else {
                return;
            };
            match range.check().await {
                Ok(()) => tracing::info!(
                    "the range at {} answers and accepts this site's token",
                    range.url
                ),
                Err(e) => tracing::warn!("the range is not usable: {e}"),
            }
        });
    }
    let app = router(state);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| fail(&format!("cannot bind {addr}: {e}")));
    tracing::info!("listening on http://{addr}");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await
    .expect("server error");
}

/// The live domain from what was typed: a bare name, with a port when there is
/// one (`necropsy.urashyl.dev`, `necropsy.localhost:8080`). A scheme and anything
/// after the host are a common slip (`https://necropsy.urashyl.dev/`), so they are
/// removed rather than being a reason not to start.
pub(crate) fn parse_live_domain(raw: &str) -> Result<String, String> {
    let mut d = raw.trim();
    for scheme in ["https://", "http://"] {
        if let Some(rest) = d.strip_prefix(scheme) {
            d = rest;
        }
    }
    let d = d.split(['/', '?', '#']).next().unwrap_or("");
    let d = d.trim().trim_matches('.').to_ascii_lowercase();
    if d.is_empty()
        || d.contains(char::is_whitespace)
        || d.contains('@')
        || (!d.contains('.') && !d.contains(':'))
    {
        return Err(format!(
            "NECROPSY_LIVE_DOMAIN {raw:?} is not a domain: use just the name, like necropsy.urashyl.dev"
        ));
    }
    Ok(d)
}

/// Both Turnstile keys, or neither.
fn captcha_from_env() -> Result<captcha::Captcha, String> {
    let get = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    match (
        get("NECROPSY_TURNSTILE_SITE_KEY"),
        get("NECROPSY_TURNSTILE_SECRET"),
    ) {
        (Some(site), Some(secret)) => Ok(captcha::Captcha::turnstile(
            site.trim().to_owned(),
            secret.trim().to_owned(),
        )),
        (None, None) => Ok(captcha::Captcha::Off),
        _ => Err(
            "set both NECROPSY_TURNSTILE_SITE_KEY and NECROPSY_TURNSTILE_SECRET, or neither".into(),
        ),
    }
}

/// `NECROPSY_LIVE_DOMAIN` turns live puzzles on; `NECROPSY_LIVE_HOST_HEADER`
/// optionally names a header that carries the original host.
fn live_from_env() -> Result<Option<live::LiveConfig>, String> {
    let Some(raw) = std::env::var("NECROPSY_LIVE_DOMAIN")
        .ok()
        .filter(|d| !d.trim().is_empty())
    else {
        return Ok(None);
    };
    let domain = parse_live_domain(&raw)?;
    if domain != raw.trim() {
        tracing::warn!(
            "NECROPSY_LIVE_DOMAIN {raw:?} is a URL, not a name: using {domain:?}. \
             Set it to just the name."
        );
    }
    let host_header = match std::env::var("NECROPSY_LIVE_HOST_HEADER") {
        Ok(h) if !h.trim().is_empty() => Some(
            h.trim()
                .parse::<axum::http::HeaderName>()
                .map_err(|_| format!("NECROPSY_LIVE_HOST_HEADER {h:?} is not a header name"))?,
        ),
        _ => None,
    };
    // Container-backed puzzles need the range VM (see docs/range.md): its
    // address and this site's token, both or neither.
    let get = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    let range = match (get("NECROPSY_RANGE_URL"), get("NECROPSY_RANGE_TOKEN")) {
        (Some(url), Some(token)) => {
            let url = url.trim();
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err(format!(
                    "NECROPSY_RANGE_URL {url:?} must start with http:// or https://"
                ));
            }
            if token.trim().len() < 16 {
                return Err("NECROPSY_RANGE_TOKEN is too short to be a token".into());
            }
            Some(live::Range::new(url, token.trim()))
        }
        (None, None) => None,
        _ => return Err("set both NECROPSY_RANGE_URL and NECROPSY_RANGE_TOKEN, or neither".into()),
    };
    Ok(Some(live::LiveConfig {
        domain,
        host_header,
        range,
    }))
}

/// `NECROPSY_CLIENT_IP_HEADER`: the header name, or unset for the peer address.
fn client_ip_header_from_env() -> Result<Option<axum::http::HeaderName>, String> {
    match std::env::var("NECROPSY_CLIENT_IP_HEADER") {
        Ok(v) if !v.trim().is_empty() => axum::http::HeaderName::from_bytes(v.trim().as_bytes())
            .map(Some)
            .map_err(|_| format!("NECROPSY_CLIENT_IP_HEADER is not a valid header name: {v}")),
        _ => Ok(None),
    }
}

/// `NECROPSY_TRUSTED_PROXIES`: comma-separated addresses or CIDR ranges.
fn trusted_proxies_from_env() -> Result<Vec<ipnet::IpNet>, String> {
    match std::env::var("NECROPSY_TRUSTED_PROXIES") {
        Ok(v) => ipnet::parse_list(&v).map_err(|e| format!("NECROPSY_TRUSTED_PROXIES: {e}")),
        Err(_) => Ok(Vec::new()),
    }
}

/// Requests `/healthz` from the server listening on `addr`. A wildcard
/// listen address is checked through loopback.
fn healthcheck(addr: SocketAddr) -> bool {
    use std::io::{Read, Write};
    use std::time::Duration;

    let mut target = addr;
    if target.ip().is_unspecified() {
        target.set_ip(if target.is_ipv4() {
            std::net::Ipv4Addr::LOCALHOST.into()
        } else {
            std::net::Ipv6Addr::LOCALHOST.into()
        });
    }
    let Ok(mut conn) = std::net::TcpStream::connect_timeout(&target, Duration::from_secs(3)) else {
        return false;
    };
    let _ = conn.set_read_timeout(Some(Duration::from_secs(3)));
    let request = b"GET /healthz HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    let mut reply = String::new();
    conn.write_all(request).is_ok()
        && conn.read_to_string(&mut reply).is_ok()
        && reply.starts_with("HTTP/1.")
        && reply.lines().next().is_some_and(|l| l.contains(" 200 "))
}

fn fail(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(1)
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_owned())
}

/// Hourly cleanup of expired sessions and tokens, accounts that never
/// verified, and stale rate-limit entries.
fn spawn_housekeeping(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(accounts::PURGE_INTERVAL);
        loop {
            tick.tick().await;
            moderation::refresh(&state, None, None).await;
            if let Err(e) = state.db.purge_expired().await {
                tracing::error!("purge failed: {e}");
            }
            state.limiter.prune(AUTH_WINDOW);
            if let Ok(active) = state.db.live_active_labels().await {
                state.hub.prune(|label| active.contains(label));
            }
        }
    });
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(pages::today))
        .route("/p/{id}", get(pages::puzzle))
        .route("/p/{id}/files/{*path}", get(pages::raw_file))
        .route("/learn", get(learn::index))
        .route(
            "/learn/path",
            post(learn::set_path).layer(DefaultBodyLimit::max(1024)),
        )
        .route("/learn/{id}", get(learn::module))
        .route(
            "/learn/{id}/checkpoint",
            post(learn::checkpoint_form).layer(DefaultBodyLimit::max(32 * 1024)),
        )
        .route(
            "/api/learn/{id}/checkpoint",
            post(learn::checkpoint_api).layer(DefaultBodyLimit::max(32 * 1024)),
        )
        .route("/api/me/learn", get(learn::progress))
        .route("/library", get(pages::library_list))
        .route("/search", get(pages::search))
        .route(
            "/live",
            get(|| async { axum::response::Redirect::permanent("/library") }),
        )
        .route("/archive", get(pages::archive))
        .route("/leaderboard", get(leaderboard::casual))
        .route("/leaderboard/ranked", get(leaderboard::ranked))
        .route(
            "/proof/{pack}",
            get(moderation::proof_page).post(moderation::proof_submit),
        )
        .route("/proof/{pack}/start", post(moderation::proof_start))
        .route("/mod/proofs", get(moderation::mod_proofs))
        .route("/mod/proofs/{id}/{action}", post(moderation::mod_review))
        .route(
            "/mod/board/{pack}/{user}/{action}",
            post(moderation::mod_board),
        )
        .route("/format", get(pages::format))
        // accounts
        .route(
            "/register",
            get(accounts::register_get).post(accounts::register_post),
        )
        .route("/verify", get(accounts::verify_get))
        .route(
            "/login",
            get(accounts::login_get).post(accounts::login_post),
        )
        .route(
            "/login/verify",
            get(mfa::login_verify_get).post(mfa::login_verify_post),
        )
        .route("/login/verify/email", post(mfa::login_verify_email_post))
        .route("/logout", post(accounts::logout_post))
        .route(
            "/forgot",
            get(accounts::forgot_get).post(accounts::forgot_post),
        )
        .route(
            "/reset",
            get(accounts::reset_get).post(accounts::reset_post),
        )
        .route("/account", get(accounts::account_get))
        .route("/account/password", post(accounts::account_password_post))
        .route("/account/username", post(accounts::account_username_post))
        .route("/account/email", post(accounts::account_email_post))
        .route("/account/delete", post(accounts::account_delete_post))
        .route("/account/mfa/totp/start", post(mfa::totp_start_post))
        .route("/account/mfa/totp/confirm", post(mfa::totp_confirm_post))
        .route("/account/mfa/totp/remove", post(mfa::totp_remove_post))
        .route("/account/mfa/email/enable", post(mfa::email_enable_post))
        .route("/account/mfa/email/disable", post(mfa::email_disable_post))
        .route("/account/mfa/recovery", post(mfa::recovery_post))
        .route("/email/confirm", get(accounts::email_confirm_get))
        // admin
        .route("/admin", get(admin::admin_get))
        .route("/admin/test-email", post(admin::test_email_post))
        .route(
            "/admin/moderators",
            get(moderation::admin_moderators).post(moderation::admin_moderators_post),
        )
        .route("/admin/audit", get(moderation::admin_audit))
        .route(
            "/admin/users/{id}/delete",
            get(admin::delete_confirm_get).post(admin::delete_post),
        )
        .route("/admin/users/{id}/{action}", post(admin::user_action_post))
        .route("/admin/live/{id}/start", post(admin::live_start_post))
        .route("/admin/live/{id}/stop", post(admin::live_stop_post))
        // json api used by app.js
        .route(
            "/api/p/{id}/answer",
            // Answers are short; a small cap keeps the endpoint cheap.
            post(check_answer).layer(DefaultBodyLimit::max(2 * 1024)),
        )
        .route("/api/p/{id}/explainer", get(explainer))
        .route(
            "/api/p/{id}/share",
            post(cards::share_post)
                .delete(cards::share_delete)
                .layer(DefaultBodyLimit::max(256)),
        )
        .route("/card/{file}", get(cards::card_get))
        .route(
            "/api/p/{id}/beat",
            post(progress::beat).layer(DefaultBodyLimit::max(256)),
        )
        .route(
            "/api/p/{id}/hint",
            post(progress::hint).layer(DefaultBodyLimit::max(256)),
        )
        .route(
            "/api/p/{id}/instance",
            get(live::instance_get).post(live::instance_post),
        )
        .route("/api/p/{id}/instance/stop", post(live::instance_stop))
        .route("/api/me/progress", get(progress))
        .route("/w/{id}", get(workspace::page))
        .route("/api/live/{label}/ws", get(wsconsole::ws))
        .route("/toolkit/{*path}", get(toolkit::file))
        .route("/static/toolkit/{file}", get(workspace::static_file))
        .route("/api/packs/wrap-key", get(sealed::wrap_key))
        .route("/api/packs/keys", get(sealed::keys))
        .route("/api/packs/{id}/key", get(sealed::key))
        .route("/healthz", get(|| async { "ok" }))
        .route("/static/app.css", get(pages::static_css))
        .route("/static/app.js", get(pages::static_js))
        .route("/static/theme.js", get(pages::static_theme_js))
        .route("/favicon.svg", get(pages::favicon))
        .fallback(pages::not_found)
        .layer(middleware::from_fn_with_state(state.clone(), origin_check))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            client_ip_layer,
        ))
        .layer(middleware::from_fn(viewer_tz))
        .layer(middleware::from_fn(security_headers))
        .layer(CompressionLayer::new())
        .layer(middleware::from_fn_with_state(state.clone(), request_log))
        .with_state(state.clone())
        // Outermost, so requests for a live instance are answered from its
        // scenario and skip everything above: the main site's CSP, cookies and
        // CSRF logic have no business on those hosts.
        .layer(middleware::from_fn_with_state(state, live_dispatch))
}

/// Makes "today" for this request the date in the viewer's timezone, which the
/// page's script keeps in the `necropsy_tz` cookie. The offset is clamped to the
/// real timezones (UTC-12 to UTC+14), so a forged cookie can only reach the date
/// in the furthest-ahead timezone: a few hours early, never days.
async fn viewer_tz(req: Request, next: Next) -> Response {
    let tz = state::parse_tz(
        req.headers()
            .get_all(header::COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .collect::<Vec<_>>()
            .join("; ")
            .as_str()
            .into(),
    );
    state::with_tz(tz, next.run(req)).await
}

/// Logs each request's method, path and status, but only when
/// `NECROPSY_LOG_REQUESTS=1`. Off by default: a site that has visitors does not
/// need a line per request, and the server logs what goes wrong without it. When
/// it is on, only the method and path are logged, never the query string, since
/// email links carry one-time tokens there.
async fn request_log(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    if !state.log_requests {
        return next.run(req).await;
    }
    let (method, path) = (req.method().clone(), req.uri().path().to_owned());
    let res = next.run(req).await;
    tracing::info!("{method} {path} {}", res.status().as_u16());
    res
}

/// Sends requests whose host is under the live domain to the live handler.
async fn live_dispatch(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    match live::live_label(&state, &req) {
        Some(label) => live::handle(state, label, req).await,
        None => next.run(req).await,
    }
}

/// Headers for every response. The CSP allows only our own scripts plus the
/// Google Fonts stylesheet and font files, which is why no page has an inline
/// script or inline style. Pages are not cached, since they can differ per
/// signed-in user; static assets set their own caching.
async fn security_headers(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    // A handler that serves something else than a page (the toolkit's files,
    // its workers) sets its own policy, and keeps it.
    h.entry(header::CONTENT_SECURITY_POLICY).or_insert(
        HeaderValue::from_static(
            "default-src 'none'; script-src 'self' https://challenges.cloudflare.com; style-src 'self' https://fonts.googleapis.com; \
             font-src https://fonts.gstatic.com; img-src 'self' data:; connect-src 'self' https://challenges.cloudflare.com; \
             frame-src https://challenges.cloudflare.com; base-uri 'none'; form-action 'self'; frame-ancestors 'none'",
        ),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    // Email links carry one-time tokens in the query string, so never send
    // the URL to another origin (the font hosts, for example).
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    h.entry(header::X_FRAME_OPTIONS)
        .or_insert(HeaderValue::from_static("DENY"));
    if !h.contains_key(header::CACHE_CONTROL) {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    res
}

/// Sets the [`ClientIp`] used for rate limiting: the connecting address, or,
/// when `NECROPSY_CLIENT_IP_HEADER` names a header, the address in that
/// header. The header is only believed when the connection comes from one of
/// `NECROPSY_TRUSTED_PROXIES` (any connection, if none are listed), and your
/// proxy must always overwrite it (nginx `proxy_set_header X-Real-IP
/// $remote_addr;`). A header holding a list, like `X-Forwarded-For`, uses its
/// last entry, the one your proxy appended. A value that is not an IP address
/// is ignored, so a client cannot make up arbitrary keys.
/// The client's address as the rate limits see it: the proxy's header when the peer
/// is a trusted proxy, otherwise the peer itself.
pub(crate) fn client_ip_for(state: &AppState, req: &Request) -> String {
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip());
    let peer_trusted = state.trusted_proxies.is_empty()
        || peer.is_some_and(|p| state.trusted_proxies.iter().any(|n| n.contains(p)));
    let from_header = state
        .client_ip_header
        .as_ref()
        .filter(|_| peer_trusted)
        .and_then(|name| {
            req.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.rsplit(',').next())
                .and_then(|s| s.trim().parse::<std::net::IpAddr>().ok())
        });
    from_header
        .or(peer)
        .map(|ip| ip.to_canonical().to_string())
        .unwrap_or_else(|| "unknown".into())
}

async fn client_ip_layer(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Response {
    let ip = client_ip_for(&state, &req);
    req.extensions_mut().insert(ClientIp(ip));
    next.run(req).await
}

/// Rejects cross-site form posts. State-changing requests must come from our
/// own origin; the session cookie is also `SameSite=Lax`. Requests with
/// neither `Origin` nor `Sec-Fetch-Site` are not browsers submitting a form
/// and cannot be a CSRF attack, so they pass.
async fn origin_check(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let safe = matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    if !safe && !same_origin(req.headers(), &state.base_url) {
        return (StatusCode::FORBIDDEN, "Cross-site request blocked.").into_response();
    }
    next.run(req).await
}

fn same_origin(headers: &HeaderMap, base_url: &str) -> bool {
    let text = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    if let Some(origin) = text("origin") {
        let Some((_, host)) = origin.split_once("://") else {
            return false; // "null" and anything malformed
        };
        let host = host.to_ascii_lowercase();
        let base_host = base_url
            .split_once("://")
            .map(|(_, h)| h.to_ascii_lowercase())
            .unwrap_or_default();
        let request_host = text("host")
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        return host == base_host || host == request_host;
    }
    match text("sec-fetch-site") {
        Some(site) => matches!(site, "same-origin" | "none"),
        None => true,
    }
}

// ---------------------------------------------------------------- json api

#[derive(Deserialize)]
struct AnswerReq {
    answer: String,
    /// What the browser counted. Used only for a solve the server has no record of,
    /// such as one imported from a browser that was signed out when it was made.
    #[serde(default)]
    seconds: i64,
    #[serde(default)]
    hints: i64,
}

#[derive(Serialize)]
struct AnswerRes {
    correct: bool,
    /// Rendered explainer HTML, only present when `correct`.
    #[serde(skip_serializing_if = "Option::is_none")]
    explainer_html: Option<String>,
    /// The signed-in player's recorded time and hints for this solve.
    #[serde(skip_serializing_if = "Option::is_none")]
    seconds: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hints: Option<i64>,
    /// On a live daily: whether this solve ended the player's ranked attempt
    /// (`ranked`) or a later one (`personal`). Absent when it ended neither, as
    /// for a flag sent in after the instance ended.
    #[serde(skip_serializing_if = "Option::is_none")]
    attempt: Option<&'static str>,
    /// Where to write the proof a ranked solve in the board's top needs
    /// (docs/moderation.md).
    #[serde(skip_serializing_if = "Option::is_none")]
    proof_url: Option<String>,
}

/// Answers one account (or, signed out, one address) may post in [`ANSWER_WINDOW`].
/// Lenient on purpose: answers are per player and long, so guessing is not the
/// threat, only a flood.
const ANSWER_MAX: usize = 10;
const ANSWER_WINDOW: std::time::Duration = std::time::Duration::from_secs(2);

async fn check_answer(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<AnswerReq>,
) -> Response {
    let Some(p) = state.get(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let who = match &viewer {
        Some(u) => format!("answer:user:{}", u.id),
        None => format!("answer:ip:{ip}"),
    };
    if !state.limiter.allow(&who, ANSWER_MAX, ANSWER_WINDOW) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            "Too many answers at once. Wait a second and try again.",
        )
            .into_response();
    }
    // No answer is anywhere near this long, so skip hashing it.
    if req.answer.len() > 512 {
        return Json(AnswerRes {
            correct: false,
            explainer_html: None,
            seconds: None,
            hints: None,
            attempt: None,
            proof_url: None,
        })
        .into_response();
    }
    let correct = if p.pack.is_live() {
        match &viewer {
            Some(u) => live::flag_matches(&state, u.id, &p.pack, &req.answer).await,
            None => false,
        }
    } else if p.pack.is_generated() {
        generated::answer_matches(&state, viewer.as_ref(), &headers, &p.pack, &req.answer).await
    } else {
        p.pack.check(&req.answer)
    };
    let mut recorded = None;
    let mut attempt = None;
    let mut proof_url = None;
    if correct {
        if let Some(user) = viewer {
            let total = p.pack.meta.hints.len() as i64;
            // The server's own time and hints, when it has been keeping them.
            let kept = state
                .db
                .progress_finish(user.id, id.clone(), progress::TIME_CAP)
                .await
                .ok()
                .flatten();
            let (seconds, hints, timed) = match kept {
                Some(k) => (k.seconds, k.hints.min(total), true),
                None => (
                    req.seconds.clamp(0, 7 * 86_400),
                    req.hints.clamp(0, total),
                    false,
                ),
            };
            if let Err(e) = state
                .db
                .record_solve(
                    user.id,
                    id.clone(),
                    seconds,
                    hints,
                    state::client_tz_min(),
                    timed,
                )
                .await
            {
                tracing::error!("recording solve failed: {e}");
            }
            state.forget_stats();
            // The attempt this instance was for ends solved, before the instance
            // is stopped below (which would end it as failed).
            if p.pack.is_live() && p.pack.meta.track == necropsy_pack::Track::Daily {
                match state.db.attempt_solved(user.id, id.clone(), hints).await {
                    Ok(a) => {
                        attempt = a.map(|a| match a {
                            db::Attempt::Ranked => "ranked",
                            db::Attempt::Personal => "personal",
                        })
                    }
                    Err(e) => tracing::error!("recording the attempt failed: {e}"),
                }
            }
            // A ranked solve may enter the board's top, and need a proof.
            if attempt == Some("ranked") {
                moderation::refresh(&state, Some(id.clone()), Some(user.id)).await;
                if let Ok(Some(p)) = state.db.proof_of(user.id, id.clone()).await {
                    if p.status == "open" {
                        proof_url = Some(format!("/proof/{id}"));
                    }
                }
            }
            // The puzzle is done, so its server is no longer needed: free the player's
            // instance (and its container) now rather than when its time runs out.
            if p.pack.is_live() {
                live::stop_instances(&state, user.id, &p.pack).await;
            }
            // Report what is on record, which is the first solve if there was one.
            recorded = state
                .db
                .solve_of(user.id, id)
                .await
                .ok()
                .flatten()
                .map(|s| (s.seconds, s.hints));
        }
    }
    Json(AnswerRes {
        correct,
        explainer_html: correct.then(|| p.explainer_html.clone()),
        seconds: recorded.map(|r| r.0),
        hints: recorded.map(|r| r.1),
        attempt,
        proof_url,
    })
    .into_response()
}

/// The explainer for a puzzle the signed-in user has already solved, so a
/// returning user on a new device gets it back without the answer.
async fn explainer(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    Path(id): Path<String>,
) -> Response {
    let (Some(p), Some(user)) = (state.get(&id), viewer) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match state.db.has_solved(user.id, id).await {
        Ok(true) => Json(AnswerRes {
            correct: true,
            explainer_html: Some(p.explainer_html.clone()),
            seconds: None,
            hints: None,
            attempt: None,
            proof_url: None,
        })
        .into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(Serialize)]
struct SolveOut {
    id: String,
    /// Days since 1970-01-01 UTC the puzzle was solved.
    day: i64,
    /// A daily solved on its own release day. Only these count for streaks, so
    /// replaying old puzzles cannot extend one.
    ontime: bool,
    seconds: i64,
    hints: i64,
    /// The server kept this solve's time, so it can have a share card.
    timed: bool,
    /// The address of the share card the player made for it, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    share_url: Option<String>,
}

#[derive(Serialize)]
struct Progress {
    /// `null` when nobody is signed in.
    user: Option<String>,
    solves: Vec<SolveOut>,
    /// Puzzles that can be answered today. The client uploads locally saved
    /// solves only for these, so a removed or not yet released puzzle is not
    /// retried on every page load.
    playable: Vec<String>,
}

/// The signed-in user's own solves. Contains no email address.
async fn progress(State(state): State<Arc<AppState>>, MaybeUser(viewer): MaybeUser) -> Response {
    let Some(user) = viewer else {
        return Json(Progress {
            user: None,
            solves: Vec::new(),
            playable: state.visible_ids(),
        })
        .into_response();
    };
    match state.db.solves_of(user.id).await {
        Ok(rows) => Json(Progress {
            user: Some(user.username),
            solves: rows
                .into_iter()
                .map(|s| {
                    // The day it was solved on is the solver's own calendar day.
                    let day = (s.solved_at + s.tz * 60).div_euclid(86_400);
                    // Visible when it was solved, so look it up without the viewer's date.
                    let ontime = state
                        .get_any(&s.pack_id)
                        .is_some_and(|p| p.pack.meta.release_day() == Some(day));
                    SolveOut {
                        id: s.pack_id,
                        day,
                        ontime,
                        seconds: s.seconds,
                        hints: s.hints,
                        timed: s.timed,
                        share_url: s.share.map(|t| format!("{}/card/{t}", state.base_url)),
                    }
                })
                .collect(),
            playable: state.visible_ids(),
        })
        .into_response(),
        Err(e) => {
            tracing::error!("loading progress failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[cfg(test)]
mod tests;
