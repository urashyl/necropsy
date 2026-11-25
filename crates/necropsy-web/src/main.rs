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
mod db;
mod ipnet;
mod live;
mod mail;
mod markdown;
mod pages;
mod state;

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
use tower_http::trace::{DefaultOnResponse, TraceLayer};
use tracing::Level;

use crate::auth::{ClientIp, MaybeUser, AUTH_WINDOW};
use crate::db::Db;
use crate::mail::Mailer;
use crate::state::{AppState, Config};

#[tokio::main]
async fn main() {
    // `necropsy-web healthcheck` is what the container's HEALTHCHECK runs, so
    // the image needs no curl or shell. It exits 0 only if /healthz answers.
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        let addr = env_or("NECROPSY_ADDR", "127.0.0.1:8080");
        let ok = addr.parse::<SocketAddr>().map(healthcheck).unwrap_or(false);
        std::process::exit(if ok { 0 } else { 1 });
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "necropsy_web=info,tower_http=info".into()),
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
    };

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

    let state = Arc::new(AppState::new(packs, config, db, mailer));
    spawn_housekeeping(state.clone());
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

/// `NECROPSY_LIVE_DOMAIN` turns live puzzles on; `NECROPSY_LIVE_HOST_HEADER`
/// optionally names a header that carries the original host.
fn live_from_env() -> Result<Option<live::LiveConfig>, String> {
    let Some(domain) = std::env::var("NECROPSY_LIVE_DOMAIN")
        .ok()
        .map(|d| d.trim().trim_matches('.').to_ascii_lowercase())
        .filter(|d| !d.is_empty())
    else {
        return Ok(None);
    };
    if domain.contains('/')
        || domain.contains(' ')
        || !domain.contains('.') && !domain.contains(':')
    {
        return Err(format!("NECROPSY_LIVE_DOMAIN {domain:?} is not a domain"));
    }
    let host_header = match std::env::var("NECROPSY_LIVE_HOST_HEADER") {
        Ok(h) if !h.trim().is_empty() => Some(
            h.trim()
                .parse::<axum::http::HeaderName>()
                .map_err(|_| format!("NECROPSY_LIVE_HOST_HEADER {h:?} is not a header name"))?,
        ),
        _ => None,
    };
    Ok(Some(live::LiveConfig {
        domain,
        host_header,
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
        .route("/learn", get(pages::learn))
        .route("/archive", get(pages::archive))
        .route("/format", get(pages::format))
        // accounts
        .route("/register", get(accounts::register_get).post(accounts::register_post))
        .route("/verify", get(accounts::verify_get))
        .route("/login", get(accounts::login_get).post(accounts::login_post))
        .route("/logout", post(accounts::logout_post))
        .route("/forgot", get(accounts::forgot_get).post(accounts::forgot_post))
        .route("/reset", get(accounts::reset_get).post(accounts::reset_post))
        .route("/account", get(accounts::account_get))
        .route("/account/password", post(accounts::account_password_post))
        .route("/account/username", post(accounts::account_username_post))
        .route("/account/email", post(accounts::account_email_post))
        .route("/account/delete", post(accounts::account_delete_post))
        .route("/email/confirm", get(accounts::email_confirm_get))
        // admin
        .route("/admin", get(admin::admin_get))
        .route("/admin/test-email", post(admin::test_email_post))
        .route(
            "/admin/users/{id}/delete",
            get(admin::delete_confirm_get).post(admin::delete_post),
        )
        .route("/admin/users/{id}/{action}", post(admin::user_action_post))
        // json api used by app.js
        .route(
            "/api/p/{id}/answer",
            // Answers are short; a small cap keeps the endpoint cheap.
            post(check_answer).layer(DefaultBodyLimit::max(2 * 1024)),
        )
        .route("/api/p/{id}/explainer", get(explainer))
        .route(
            "/api/p/{id}/instance",
            get(live::instance_get).post(live::instance_post),
        )
        .route("/api/p/{id}/instance/stop", post(live::instance_stop))
        .route("/api/me/progress", get(progress))
        .route("/healthz", get(|| async { "ok" }))
        .route("/static/app.css", get(pages::static_css))
        .route("/static/app.js", get(pages::static_js))
        .route("/static/theme.js", get(pages::static_theme_js))
        .route("/favicon.svg", get(pages::favicon))
        .fallback(pages::not_found)
        .layer(middleware::from_fn_with_state(state.clone(), origin_check))
        .layer(middleware::from_fn_with_state(state.clone(), client_ip_layer))
        .layer(middleware::from_fn(security_headers))
        .layer(CompressionLayer::new())
        .layer(
            // Log the method and path only. Query strings carry one-time
            // tokens (email links) and must not reach the log.
            TraceLayer::new_for_http()
                .make_span_with(|req: &Request| {
                    tracing::info_span!("http", method = %req.method(), path = %req.uri().path())
                })
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .with_state(state.clone())
        // Outermost, so requests for a live instance are answered from its
        // scenario and skip everything above: the main site's CSP, cookies and
        // CSRF logic have no business on those hosts.
        .layer(middleware::from_fn_with_state(state, live_dispatch))
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
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; script-src 'self'; style-src 'self' https://fonts.googleapis.com; \
             font-src https://fonts.gstatic.com; img-src 'self' data:; connect-src 'self'; \
             base-uri 'none'; form-action 'self'; frame-ancestors 'none'",
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
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
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
async fn client_ip_layer(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Response {
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
    let ip = from_header
        .or(peer)
        .map(|ip| ip.to_canonical().to_string())
        .unwrap_or_else(|| "unknown".into());
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
    /// Reported by the browser for the signed-in user's own history.
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
}

async fn check_answer(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    Path(id): Path<String>,
    Json(req): Json<AnswerReq>,
) -> Response {
    let Some(p) = state.get(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    // No answer is anywhere near this long, so skip hashing it.
    if req.answer.len() > 512 {
        return Json(AnswerRes {
            correct: false,
            explainer_html: None,
        })
        .into_response();
    }
    let correct = if p.pack.is_live() {
        match &viewer {
            Some(u) => live::flag_matches(&state, u.id, &p.pack, &req.answer).await,
            None => false,
        }
    } else {
        p.pack.check(&req.answer)
    };
    if correct {
        if let Some(user) = viewer {
            let seconds = req.seconds.clamp(0, 7 * 86_400);
            let hints = req.hints.clamp(0, p.pack.meta.hints.len() as i64);
            if let Err(e) = state.db.record_solve(user.id, id, seconds, hints).await {
                tracing::error!("recording solve failed: {e}");
            }
        }
    }
    Json(AnswerRes {
        correct,
        explainer_html: correct.then(|| p.explainer_html.clone()),
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
                    let day = s.solved_at.div_euclid(86_400);
                    let ontime = state
                        .get(&s.pack_id)
                        .is_some_and(|p| p.pack.meta.release_day() == Some(day));
                    SolveOut {
                        id: s.pack_id,
                        day,
                        ontime,
                        seconds: s.seconds,
                        hints: s.hints,
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
