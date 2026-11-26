// SPDX-License-Identifier: AGPL-3.0-or-later
//! End-to-end tests through the real router, with an in-memory database and
//! an in-memory mailer.

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum::Router;
use tower::ServiceExt;

use crate::auth;
use crate::db::Db;
use crate::mail::{Mail, Mailer};
use crate::router;
use crate::state::{AppState, Config};

const ORIGIN: &str = "https://example.test";
const PASSWORD: &str = "correct horse battery";
const SECRET_EMAIL: &str = "alice-secret@example.org";

struct Site {
    app: Router,
    state: Arc<AppState>,
    mail: Arc<Mutex<Vec<Mail>>>,
}

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
}

impl Reply {
    fn cookie(&self) -> Option<String> {
        self.headers
            .get(header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .filter(|v| !v.ends_with('='))
            .map(str::to_owned)
    }
}

async fn build(mailer: Option<Mailer>, client_ip_header: Option<&str>) -> Site {
    build_trusting(mailer, client_ip_header, &[]).await
}

/// Every daily puzzle is released by this day.
const ALL_RELEASED: &str = "2026-12-08";

async fn build_trusting(
    mailer: Option<Mailer>,
    client_ip_header: Option<&str>,
    trusted: &[&str],
) -> Site {
    build_on(mailer, client_ip_header, trusted, ALL_RELEASED).await
}

/// A site whose clock says it is `today` (YYYY-MM-DD).
async fn build_on(
    mailer: Option<Mailer>,
    client_ip_header: Option<&str>,
    trusted: &[&str],
    today: &str,
) -> Site {
    build_full(
        mailer,
        client_ip_header,
        trusted,
        today,
        None,
        crate::captcha::Captcha::Off,
    )
    .await
}

const LIVE_DOMAIN: &str = "live.example.test";

async fn build_full(
    mailer: Option<Mailer>,
    client_ip_header: Option<&str>,
    trusted: &[&str],
    today: &str,
    live: Option<crate::live::LiveConfig>,
    captcha: crate::captcha::Captcha,
) -> Site {
    build_logging(
        mailer,
        client_ip_header,
        trusted,
        today,
        live,
        captcha,
        false,
    )
    .await
}

/// `build_full`, and whether the site logs each request.
async fn build_logging(
    mailer: Option<Mailer>,
    client_ip_header: Option<&str>,
    trusted: &[&str],
    today: &str,
    live: Option<crate::live::LiveConfig>,
    captcha: crate::captcha::Captcha,
    log_requests: bool,
) -> Site {
    let packs = necropsy_pack::load_all(std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../packs"
    )))
    .unwrap();
    let mail = Arc::new(Mutex::new(Vec::new()));
    let mailer = mailer.unwrap_or_else(|| Mailer::Memory(mail.clone()));
    let mut state = AppState::new(
        packs,
        Config {
            base_url: ORIGIN.into(),
            client_ip_header: client_ip_header.map(|h| h.parse().unwrap()),
            trusted_proxies: trusted.iter().map(|n| n.parse().unwrap()).collect(),
            live,
            captcha,
            log_requests,
        },
        Db::open(":memory:").unwrap(),
        mailer,
    );
    state.today_override = Some(necropsy_pack::parse_date(today).unwrap());
    let state = Arc::new(state);
    Site {
        app: router(state.clone()),
        state,
        mail,
    }
}

async fn site_on(today: &str) -> Site {
    build_on(None, None, &[], today).await
}

async fn site() -> Site {
    build(None, None).await
}

async fn site_with(mailer: Option<Mailer>) -> Site {
    build(mailer, None).await
}

impl Site {
    async fn send(&self, req: Request<Body>) -> Reply {
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        let bytes = axum::body::to_bytes(res.into_body(), 4 << 20)
            .await
            .unwrap();
        Reply {
            status,
            headers,
            body: String::from_utf8_lossy(&bytes).into_owned(),
        }
    }

    async fn get(&self, path: &str, cookie: Option<&str>) -> Reply {
        let mut b = Request::get(path);
        if let Some(c) = cookie {
            b = b.header(header::COOKIE, c);
        }
        self.send(b.body(Body::empty()).unwrap()).await
    }

    async fn post(&self, path: &str, fields: &[(&str, &str)], cookie: Option<&str>) -> Reply {
        let body = fields
            .iter()
            .map(|(k, v)| format!("{k}={}", urlenc(v)))
            .collect::<Vec<_>>()
            .join("&");
        let mut b = Request::post(path)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(header::ORIGIN, ORIGIN);
        if let Some(c) = cookie {
            b = b.header(header::COOKIE, c);
        }
        self.send(b.body(Body::from(body)).unwrap()).await
    }

    async fn post_json(&self, path: &str, json: &str, cookie: Option<&str>) -> Reply {
        let mut b = Request::post(path)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ORIGIN, ORIGIN);
        if let Some(c) = cookie {
            b = b.header(header::COOKIE, c);
        }
        self.send(b.body(Body::from(json.to_owned())).unwrap())
            .await
    }

    /// Registers, then follows the emailed verification link.
    async fn register_verified(&self, username: &str, email: &str) {
        let r = self
            .post(
                "/register",
                &[
                    ("username", username),
                    ("email", email),
                    ("password", PASSWORD),
                ],
                None,
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.body);
        let link = self.last_link("/verify?token=");
        assert_eq!(self.get(&link, None).await.status, StatusCode::OK);
    }

    async fn login(&self, username: &str, password: &str) -> Reply {
        self.post(
            "/login",
            &[("username", username), ("password", password)],
            None,
        )
        .await
    }

    async fn login_cookie(&self, username: &str) -> String {
        let r = self.login(username, PASSWORD).await;
        assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
        r.cookie().expect("session cookie")
    }

    /// The path-and-query of the newest emailed link containing `marker`.
    fn last_link(&self, marker: &str) -> String {
        let mail = self.mail.lock().unwrap();
        let body = &mail
            .iter()
            .rev()
            .find(|m| m.body.contains(marker))
            .unwrap_or_else(|| panic!("no mail with {marker}"))
            .body;
        let start = body.find(ORIGIN).unwrap() + ORIGIN.len();
        body[start..].split_whitespace().next().unwrap().to_owned()
    }

    fn mail_count(&self) -> usize {
        self.mail.lock().unwrap().len()
    }

    /// Creates the admin and signs it in.
    async fn admin_cookie(&self) -> String {
        let (user, pass) = auth::bootstrap_admin(&self.state.db)
            .await
            .unwrap()
            .unwrap();
        let r = self.login(&user, &pass).await;
        assert_eq!(r.status, StatusCode::SEE_OTHER);
        r.cookie().unwrap()
    }
}

fn urlenc(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

// ---------------------------------------------------------------- site basics

#[tokio::test]
async fn pages_render() {
    let s = site().await;
    for path in [
        "/",
        "/archive",
        "/learn",
        "/format",
        "/login",
        "/register",
        "/p/intro-log",
        "/p/peanuts",
    ] {
        let r = s.get(path, None).await;
        assert_eq!(r.status, StatusCode::OK, "{path}");
        assert!(r.body.contains("<title>"), "{path}");
    }
    assert_eq!(s.get("/p/nope", None).await.status, StatusCode::NOT_FOUND);
    assert_eq!(s.get("/nope", None).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn puzzle_page_never_contains_explainer_or_answer() {
    let r = site().await.get("/p/intro-log", None).await;
    assert!(!r.body.contains("The answer is"));
}

#[tokio::test]
async fn wrong_answer_gets_no_explainer_and_right_answer_does() {
    let s = site().await;
    let r = s
        .post_json(
            "/api/p/intro-log/answer",
            r#"{"answer":"192.0.2.15"}"#,
            None,
        )
        .await;
    assert!(r.body.contains("\"correct\":false") && !r.body.contains("explainer_html"));
    let r = s
        .post_json(
            "/api/p/intro-log/answer",
            r#"{"answer":"  203.0.113.77 "}"#,
            None,
        )
        .await;
    assert!(r.body.contains("\"correct\":true") && r.body.contains("explainer_html"));
    assert_eq!(
        s.post_json("/api/p/nope/answer", r#"{"answer":"x"}"#, None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn raw_files_are_limited_to_the_pack() {
    let s = site().await;
    let r = s.get("/p/peanuts/files/cookies.txt", None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.contains("session_token"));
    for bad in [
        "/p/peanuts/files/../answer",
        "/p/peanuts/files/%2e%2e/answer",
        "/p/peanuts/files/missing.txt",
    ] {
        assert_eq!(
            s.get(bad, None).await.status,
            StatusCode::NOT_FOUND,
            "{bad}"
        );
    }
}

#[tokio::test]
async fn security_headers_present() {
    let r = site().await.get("/", None).await;
    assert!(r.headers.contains_key(header::CONTENT_SECURITY_POLICY));
    assert_eq!(r.headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert_eq!(r.headers[header::REFERRER_POLICY], "same-origin");
}

// ---------------------------------------------------------------- admin bootstrap

#[tokio::test]
async fn admin_is_generated_once_and_has_no_email() {
    let s = site().await;
    let (user, pass) = auth::bootstrap_admin(&s.state.db).await.unwrap().unwrap();
    assert!(user.starts_with("admin-") && user.len() == 12);
    assert_eq!(pass.len(), 24);
    assert!(auth::bootstrap_admin(&s.state.db).await.unwrap().is_none());

    let r = s.login(&user, &pass).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let id = s.state.db.user_for_login(user).await.unwrap().unwrap().0.id;
    assert_eq!(s.state.db.email_of(id).await.unwrap(), None);
}

#[tokio::test]
async fn generated_credentials_differ_between_installs() {
    let (a, b) = (site().await, site().await);
    let ca = auth::bootstrap_admin(&a.state.db).await.unwrap().unwrap();
    let cb = auth::bootstrap_admin(&b.state.db).await.unwrap().unwrap();
    assert_ne!(ca.1, cb.1);
}

// ---------------------------------------------------------------- accounts

#[tokio::test]
async fn register_verify_login_and_email_is_never_rendered() {
    let s = site().await;
    let r = s
        .post(
            "/register",
            &[
                ("username", "alice"),
                ("email", SECRET_EMAIL),
                ("password", PASSWORD),
            ],
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(!r.body.contains("alice-secret"));

    // Not verified yet.
    assert_eq!(
        s.login("alice", PASSWORD).await.status,
        StatusCode::FORBIDDEN
    );

    let link = s.last_link("/verify?token=");
    assert_eq!(s.get(&link, None).await.status, StatusCode::OK);
    // One-time link.
    assert_eq!(s.get(&link, None).await.status, StatusCode::BAD_REQUEST);

    let cookie = s.login_cookie("alice").await;
    let account = s.get("/account", Some(&cookie)).await;
    assert_eq!(account.status, StatusCode::OK);
    assert!(account.body.contains("alice"));

    let admin = s.admin_cookie().await;
    let mut pages = vec![
        account.body,
        s.get("/admin", Some(&admin)).await.body,
        s.get("/api/me/progress", Some(&cookie)).await.body,
        s.get("/", Some(&cookie)).await.body,
    ];
    assert!(pages[1].contains("alice"), "admin list shows the username");
    pages.push(s.get("/archive", Some(&admin)).await.body);
    for body in pages {
        assert!(!body.contains("alice-secret"), "email leaked into a page");
        assert!(
            !body.contains("example.org"),
            "email domain leaked into a page"
        );
    }
}

#[tokio::test]
async fn sessions_are_stored_hashed() {
    let s = site().await;
    s.register_verified("alice", SECRET_EMAIL).await;
    let cookie = s.login_cookie("alice").await;
    let token = cookie.split_once('=').unwrap().1.to_owned();
    let stored: Vec<String> = s
        .state
        .db
        .run(|c| {
            let mut st = c.prepare("SELECT token_hash FROM sessions")?;
            let rows = st.query_map([], |r| r.get(0))?;
            rows.collect()
        })
        .await
        .unwrap();
    assert!(!stored.is_empty() && !stored.contains(&token));
    assert!(stored.contains(&auth::token_hash(&token)));
}

#[tokio::test]
async fn wrong_password_and_unknown_user_look_the_same() {
    let s = site().await;
    s.register_verified("alice", SECRET_EMAIL).await;
    let a = s.login("alice", "not the password").await;
    let b = s.login("nobody", "not the password").await;
    assert_eq!(a.status, StatusCode::UNAUTHORIZED);
    assert_eq!(b.status, StatusCode::UNAUTHORIZED);
    assert_eq!(a.body, b.body);
}

#[tokio::test]
async fn duplicate_username_is_rejected_and_duplicate_email_is_silent() {
    let s = site().await;
    s.register_verified("alice", SECRET_EMAIL).await;
    let before = s.mail_count();

    let r = s
        .post(
            "/register",
            &[
                ("username", "ALICE"),
                ("email", "other@example.org"),
                ("password", PASSWORD),
            ],
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);

    let r = s
        .post(
            "/register",
            &[
                ("username", "bob"),
                ("email", SECRET_EMAIL),
                ("password", PASSWORD),
            ],
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "same reply as a fresh address");
    assert_eq!(
        s.mail_count(),
        before,
        "no mail goes to an address that already has an account"
    );
    assert_eq!(
        s.login("bob", PASSWORD).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn bad_registration_input_is_rejected() {
    let s = site().await;
    for (u, e, p) in [
        ("ab", "a@example.org", PASSWORD),
        ("admin-evil", "a@example.org", PASSWORD),
        ("carol", "not-an-email", PASSWORD),
        ("carol", "a@example.org", "short"),
    ] {
        let r = s
            .post(
                "/register",
                &[("username", u), ("email", e), ("password", p)],
                None,
            )
            .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{u} {e}");
    }
    assert_eq!(s.mail_count(), 0);
}

#[tokio::test]
async fn password_reset_flow() {
    let s = site().await;
    s.register_verified("alice", SECRET_EMAIL).await;

    // An unknown address gets the same reply and no mail.
    let before = s.mail_count();
    let unknown = s
        .post("/forgot", &[("email", "nobody@example.org")], None)
        .await;
    let known = s.post("/forgot", &[("email", SECRET_EMAIL)], None).await;
    assert_eq!(unknown.status, known.status);
    assert_eq!(unknown.body, known.body);
    assert_eq!(s.mail_count(), before + 1);

    let link = s.last_link("/reset?token=");
    let token = link.split_once("token=").unwrap().1.to_owned();
    assert_eq!(s.get(&link, None).await.status, StatusCode::OK);

    let old_cookie = s.login_cookie("alice").await;
    let r = s
        .post(
            "/reset",
            &[("token", &token), ("password", "a brand new password")],
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);

    assert_eq!(
        s.login("alice", PASSWORD).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        s.login("alice", "a brand new password").await.status,
        StatusCode::SEE_OTHER
    );
    // Existing sessions end, and the link works once.
    assert_eq!(
        s.get("/account", Some(&old_cookie)).await.status,
        StatusCode::SEE_OTHER
    );
    let r = s
        .post(
            "/reset",
            &[("token", &token), ("password", "yet another password")],
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn change_password_keeps_this_session_and_ends_others() {
    let s = site().await;
    s.register_verified("alice", SECRET_EMAIL).await;
    let phone = s.login_cookie("alice").await;
    let laptop = s.login_cookie("alice").await;

    let bad = s
        .post(
            "/account/password",
            &[("current", "wrong"), ("password", "a brand new password")],
            Some(&laptop),
        )
        .await;
    assert_eq!(bad.status, StatusCode::FORBIDDEN);

    let ok = s
        .post(
            "/account/password",
            &[("current", PASSWORD), ("password", "a brand new password")],
            Some(&laptop),
        )
        .await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.body);
    let fresh = ok.cookie().expect("fresh session");
    assert_eq!(s.get("/account", Some(&fresh)).await.status, StatusCode::OK);
    assert_eq!(
        s.get("/account", Some(&phone)).await.status,
        StatusCode::SEE_OTHER
    );
}

#[tokio::test]
async fn change_email_needs_confirmation_and_never_shows_it() {
    let s = site().await;
    s.register_verified("alice", SECRET_EMAIL).await;
    let cookie = s.login_cookie("alice").await;

    let r = s
        .post(
            "/account/email",
            &[("email", "new-address@example.org"), ("password", PASSWORD)],
            Some(&cookie),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(!r.body.contains("new-address"));

    let link = s.last_link("/email/confirm?token=");
    assert_eq!(s.get(&link, Some(&cookie)).await.status, StatusCode::OK);

    // The new address now works for password reset; the old one does not.
    let before = s.mail_count();
    s.post("/forgot", &[("email", SECRET_EMAIL)], None).await;
    assert_eq!(s.mail_count(), before);
    s.post("/forgot", &[("email", "new-address@example.org")], None)
        .await;
    assert_eq!(s.mail_count(), before + 1);
}

#[tokio::test]
async fn delete_account_removes_progress() {
    let s = site().await;
    s.register_verified("alice", SECRET_EMAIL).await;
    let cookie = s.login_cookie("alice").await;
    s.post_json(
        "/api/p/peanuts/answer",
        r#"{"answer":"sess_9f2c41ab77e0"}"#,
        Some(&cookie),
    )
    .await;

    assert_eq!(
        s.post("/account/delete", &[("password", "wrong")], Some(&cookie))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let r = s
        .post("/account/delete", &[("password", PASSWORD)], Some(&cookie))
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(
        s.login("alice", PASSWORD).await.status,
        StatusCode::UNAUTHORIZED
    );
    let left: i64 = s
        .state
        .db
        .run(|c| c.query_row("SELECT COUNT(*) FROM solves", [], |r| r.get(0)))
        .await
        .unwrap();
    assert_eq!(left, 0);
}

#[tokio::test]
async fn registration_and_reset_are_closed_without_email() {
    let s = site_with(Some(Mailer::Disabled)).await;
    for path in ["/register", "/forgot"] {
        assert_eq!(
            s.get(path, None).await.status,
            StatusCode::SERVICE_UNAVAILABLE,
            "{path}"
        );
    }
    let r = s
        .post(
            "/register",
            &[
                ("username", "alice"),
                ("email", SECRET_EMAIL),
                ("password", PASSWORD),
            ],
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE);
    // The admin can still sign in.
    let _ = s.admin_cookie().await;
}

#[tokio::test]
async fn login_is_rate_limited_per_username() {
    let s = site().await;
    for _ in 0..10 {
        assert_eq!(
            s.login("alice", "guess").await.status,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        s.login("alice", "guess").await.status,
        StatusCode::TOO_MANY_REQUESTS
    );
}

// ---------------------------------------------------------------- csrf

#[tokio::test]
async fn cross_site_posts_are_blocked() {
    let s = site().await;
    let post = |origin: Option<&str>, fetch_site: Option<&str>| {
        let mut b = Request::post("/login")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
        if let Some(o) = origin {
            b = b.header(header::ORIGIN, o);
        }
        if let Some(f) = fetch_site {
            b = b.header("sec-fetch-site", f);
        }
        b.body(Body::from("username=a&password=b")).unwrap()
    };
    assert_eq!(
        s.send(post(Some("https://evil.test"), None)).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        s.send(post(Some("null"), None)).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        s.send(post(None, Some("cross-site"))).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        s.send(post(Some(ORIGIN), None)).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        s.send(post(None, Some("same-origin"))).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[test]
fn same_origin_checks_host_and_base() {
    let mut h = HeaderMap::new();
    h.insert(header::ORIGIN, "http://localhost:8080".parse().unwrap());
    h.insert(header::HOST, "localhost:8080".parse().unwrap());
    assert!(crate::same_origin(&h, "https://necropsy.urashyl.dev"));
    h.insert(header::HOST, "evil.test".parse().unwrap());
    assert!(!crate::same_origin(&h, "https://necropsy.urashyl.dev"));
}

// ---------------------------------------------------------------- progress

#[tokio::test]
async fn progress_lists_only_what_can_be_answered_today() {
    // The client uploads local solves only for these, so a removed or not
    // yet released puzzle is not posted on every page load.
    let s = site_on("2026-11-25").await;
    s.register_verified("alice", SECRET_EMAIL).await;
    let cookie = s.login_cookie("alice").await;
    for cookie in [Some(cookie.as_str()), None] {
        let body = s.get("/api/me/progress", cookie).await.body;
        let start = body.find("\"playable\":[").expect("playable list") + 12;
        let list = &body[start..start + body[start..].find(']').unwrap()];
        let ids: Vec<&str> = list.split(',').map(|x| x.trim_matches('"')).collect();
        for id in ["intro-log", "iloveyou-dropped", "iloveyou-attachment"] {
            assert!(ids.contains(&id), "{id} in {ids:?}");
        }
        for id in ["iloveyou-payload", "peanuts", "second-stage", "phone-home"] {
            assert!(!ids.contains(&id), "{id} in {ids:?}");
        }
    }
}

#[tokio::test]
async fn solves_are_recorded_for_signed_in_users() {
    let s = site().await;
    s.register_verified("alice", SECRET_EMAIL).await;
    let cookie = s.login_cookie("alice").await;

    let none = s.get("/api/me/progress", Some(&cookie)).await;
    assert!(none.body.contains("\"user\":\"alice\"") && none.body.contains("\"solves\":[]"));
    assert!(s
        .get("/api/me/progress", None)
        .await
        .body
        .contains("\"user\":null"));

    // A wrong answer records nothing; a right one does, once.
    s.post_json(
        "/api/p/peanuts/answer",
        r#"{"answer":"nope"}"#,
        Some(&cookie),
    )
    .await;
    for _ in 0..2 {
        s.post_json(
            "/api/p/peanuts/answer",
            r#"{"answer":"sess_9f2c41ab77e0","seconds":95,"hints":1}"#,
            Some(&cookie),
        )
        .await;
    }
    let p = s.get("/api/me/progress", Some(&cookie)).await.body;
    assert_eq!(p.matches("\"id\":\"peanuts\"").count(), 1, "{p}");
    assert!(p.contains("\"seconds\":95") && p.contains("\"hints\":1"));

    // The explainer comes back for a solved puzzle, not an unsolved one.
    assert_eq!(
        s.get("/api/p/peanuts/explainer", Some(&cookie))
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        s.get("/api/p/intro-log/explainer", Some(&cookie))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        s.get("/api/p/peanuts/explainer", None).await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn reported_time_and_hints_are_clamped() {
    let s = site().await;
    s.register_verified("alice", SECRET_EMAIL).await;
    let cookie = s.login_cookie("alice").await;
    s.post_json(
        "/api/p/peanuts/answer",
        r#"{"answer":"sess_9f2c41ab77e0","seconds":99999999999,"hints":50}"#,
        Some(&cookie),
    )
    .await;
    let p = s.get("/api/me/progress", Some(&cookie)).await.body;
    assert!(
        p.contains("\"seconds\":604800") && p.contains("\"hints\":2"),
        "{p}"
    );
}

// ---------------------------------------------------------------- admin

#[tokio::test]
async fn admin_page_is_admin_only() {
    let s = site().await;
    s.register_verified("alice", SECRET_EMAIL).await;
    let alice = s.login_cookie("alice").await;
    let admin = s.admin_cookie().await;

    let anon = s.get("/admin", None).await;
    assert_eq!(anon.status, StatusCode::SEE_OTHER);
    assert_eq!(
        s.get("/admin", Some(&alice)).await.status,
        StatusCode::NOT_FOUND
    );
    let r = s.get("/admin", Some(&admin)).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.contains("alice") && !r.body.contains("Not configured"));

    let id = s
        .state
        .db
        .user_for_login("alice".into())
        .await
        .unwrap()
        .unwrap()
        .0
        .id;
    for (method_path, who) in [
        (format!("/admin/users/{id}/disable"), &alice),
        (format!("/admin/users/{id}/delete"), &alice),
        ("/admin/test-email".to_owned(), &alice),
    ] {
        let r = s
            .post(&method_path, &[("to", "x@example.org")], Some(who))
            .await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{method_path}");
    }
}

#[tokio::test]
async fn admin_can_disable_enable_and_delete_users() {
    let s = site().await;
    s.register_verified("alice", SECRET_EMAIL).await;
    let alice = s.login_cookie("alice").await;
    let admin = s.admin_cookie().await;
    let id = s
        .state
        .db
        .user_for_login("alice".into())
        .await
        .unwrap()
        .unwrap()
        .0
        .id;

    let r = s
        .post(&format!("/admin/users/{id}/disable"), &[], Some(&admin))
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(
        s.get("/account", Some(&alice)).await.status,
        StatusCode::SEE_OTHER,
        "session ended"
    );
    assert_eq!(
        s.login("alice", PASSWORD).await.status,
        StatusCode::FORBIDDEN
    );

    s.post(&format!("/admin/users/{id}/enable"), &[], Some(&admin))
        .await;
    assert_eq!(
        s.login("alice", PASSWORD).await.status,
        StatusCode::SEE_OTHER
    );

    assert_eq!(
        s.get(&format!("/admin/users/{id}/delete"), Some(&admin))
            .await
            .status,
        StatusCode::OK
    );
    s.post(&format!("/admin/users/{id}/delete"), &[], Some(&admin))
        .await;
    assert_eq!(
        s.login("alice", PASSWORD).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn admin_cannot_touch_admin_accounts() {
    let s = site().await;
    let admin = s.admin_cookie().await;
    let id = s
        .state
        .db
        .run(|c| {
            c.query_row("SELECT id FROM users WHERE is_admin = 1", [], |r| {
                r.get::<_, i64>(0)
            })
        })
        .await
        .unwrap();
    for path in [
        format!("/admin/users/{id}/disable"),
        format!("/admin/users/{id}/delete"),
    ] {
        assert_eq!(
            s.post(&path, &[], Some(&admin)).await.status,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    assert_eq!(
        s.post("/account/delete", &[("password", "x")], Some(&admin))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn admin_test_email_sends() {
    let s = site().await;
    let admin = s.admin_cookie().await;
    let r = s
        .post(
            "/admin/test-email",
            &[("to", "someone@example.org")],
            Some(&admin),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(
        s.mail.lock().unwrap().last().unwrap().to,
        "someone@example.org"
    );
}

#[tokio::test]
async fn unverified_accounts_are_purged_after_two_days() {
    let s = site().await;
    s.post(
        "/register",
        &[
            ("username", "ghost"),
            ("email", "ghost@example.org"),
            ("password", PASSWORD),
        ],
        None,
    )
    .await;
    s.state
        .db
        .run(|c| c.execute("UPDATE users SET created_at = created_at - 200000", []))
        .await
        .unwrap();
    s.state.db.purge_expired().await.unwrap();
    assert!(s
        .state
        .db
        .user_for_login("ghost".into())
        .await
        .unwrap()
        .is_none());
}

// ---------------------------------------------------------------- client address

fn login_from(ip: Option<&str>, header: &str, user: &str) -> Request<Body> {
    let mut b = Request::post("/login")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::ORIGIN, ORIGIN);
    if let Some(ip) = ip {
        b = b.header(header, ip);
    }
    b.body(Body::from(format!("username={user}&password=x")))
        .unwrap()
}

#[tokio::test]
async fn rate_limit_uses_the_configured_client_ip_header() {
    let s = build(None, Some("X-Real-IP")).await;
    // 30 attempts per address, each with a different username so only the
    // per-address limit can trip.
    for i in 0..30 {
        let r = s
            .send(login_from(
                Some("203.0.113.7"),
                "x-real-ip",
                &format!("user{i}"),
            ))
            .await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "attempt {i}");
    }
    let blocked = s
        .send(login_from(Some("203.0.113.7"), "x-real-ip", "user99"))
        .await;
    assert_eq!(blocked.status, StatusCode::TOO_MANY_REQUESTS);
    // A different address is unaffected.
    let other = s
        .send(login_from(Some("198.51.100.9"), "x-real-ip", "user99"))
        .await;
    assert_eq!(other.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn client_ip_header_is_ignored_unless_configured_and_valid() {
    // Not configured: the header is not trusted, so every request shares the
    // same fallback key and a spoofed header does not dodge the limit.
    let s = build(None, None).await;
    for i in 0..30 {
        s.send(login_from(
            Some(&format!("10.0.0.{i}")),
            "x-real-ip",
            &format!("u{i}"),
        ))
        .await;
    }
    let r = s
        .send(login_from(Some("10.9.9.9"), "x-real-ip", "u99"))
        .await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);

    // Configured, but the value is not an IP address: ignored the same way.
    let s = build(None, Some("x-real-ip")).await;
    for i in 0..30 {
        s.send(login_from(
            Some(&format!("not-an-ip-{i}")),
            "x-real-ip",
            &format!("u{i}"),
        ))
        .await;
    }
    let r = s
        .send(login_from(Some("also-garbage"), "x-real-ip", "u99"))
        .await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn list_headers_use_the_last_entry() {
    let s = build(None, Some("x-forwarded-for")).await;
    for i in 0..30 {
        // The client-supplied first entry varies; the proxy-appended last one does not.
        let r = s
            .send(login_from(
                Some(&format!("1.2.3.{i}, 203.0.113.7")),
                "x-forwarded-for",
                &format!("u{i}"),
            ))
            .await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    }
    let r = s
        .send(login_from(
            Some("9.9.9.9, 203.0.113.7"),
            "x-forwarded-for",
            "u99",
        ))
        .await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
}

/// Pretends the request arrived over a connection from `peer`.
fn from_peer(mut req: Request<Body>, peer: &str) -> Request<Body> {
    let addr: std::net::SocketAddr = peer.parse().unwrap();
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(addr));
    req
}

#[tokio::test]
async fn header_is_only_believed_from_trusted_proxies() {
    let s = build_trusting(None, Some("x-real-ip"), &["127.0.0.1", "10.1.0.0/16"]).await;

    // From a trusted proxy the header decides the key: one address runs out
    // while another is unaffected.
    for i in 0..30 {
        let r = s
            .send(from_peer(
                login_from(Some("203.0.113.7"), "x-real-ip", &format!("u{i}")),
                "127.0.0.1:5000",
            ))
            .await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{i}");
    }
    let r = s
        .send(from_peer(
            login_from(Some("203.0.113.7"), "x-real-ip", "u99"),
            "10.1.2.3:5000",
        ))
        .await;
    assert_eq!(
        r.status,
        StatusCode::TOO_MANY_REQUESTS,
        "range member is trusted too"
    );
    let r = s
        .send(from_peer(
            login_from(Some("198.51.100.9"), "x-real-ip", "u99"),
            "127.0.0.1:5000",
        ))
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);

    // From anyone else the header is ignored, so changing it does not help:
    // every request counts against the connecting address.
    for i in 0..30 {
        let r = s
            .send(from_peer(
                login_from(Some(&format!("192.0.2.{i}")), "x-real-ip", &format!("v{i}")),
                "198.18.0.5:4000",
            ))
            .await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{i}");
    }
    let r = s
        .send(from_peer(
            login_from(Some("192.0.2.200"), "x-real-ip", "v99"),
            "198.18.0.5:4000",
        ))
        .await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn untrusted_when_the_peer_is_unknown() {
    // With a trusted list, a request with no known peer address is not trusted.
    let s = build_trusting(None, Some("x-real-ip"), &["127.0.0.1"]).await;
    for i in 0..30 {
        s.send(login_from(
            Some(&format!("192.0.2.{i}")),
            "x-real-ip",
            &format!("w{i}"),
        ))
        .await;
    }
    let r = s
        .send(login_from(Some("192.0.2.200"), "x-real-ip", "w99"))
        .await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
}

// ---------------------------------------------------------------- admin identity

#[tokio::test]
async fn admin_can_rename_themselves_even_to_a_reserved_name() {
    let s = site().await;
    let (generated, pass) = auth::bootstrap_admin(&s.state.db).await.unwrap().unwrap();
    let cookie = s.login(&generated, &pass).await.cookie().unwrap();

    // Password required, format enforced.
    let wrong = s
        .post(
            "/account/username",
            &[("username", "admin"), ("password", "nope")],
            Some(&cookie),
        )
        .await;
    assert_eq!(wrong.status, StatusCode::FORBIDDEN);
    let bad = s
        .post(
            "/account/username",
            &[("username", "no spaces"), ("password", pass.as_str())],
            Some(&cookie),
        )
        .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);

    let ok = s
        .post(
            "/account/username",
            &[("username", "admin"), ("password", pass.as_str())],
            Some(&cookie),
        )
        .await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.body);
    assert!(ok.body.contains("Username changed"));
    // The session carries over, and only the new name signs in.
    let page = s.get("/account", Some(&cookie)).await;
    assert!(
        page.body.contains(">admin<"),
        "header and account show the new name"
    );
    assert_eq!(
        s.login(&generated, &pass).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(s.login("admin", &pass).await.status, StatusCode::SEE_OTHER);

    // Regular users still cannot register it.
    let r = s
        .post(
            "/register",
            &[
                ("username", "admin"),
                ("email", "x@example.org"),
                ("password", PASSWORD),
            ],
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn admin_username_cannot_collide_and_others_cannot_rename() {
    let s = site().await;
    s.register_verified("alice", SECRET_EMAIL).await;
    let alice = s.login_cookie("alice").await;
    let (user, pass) = auth::bootstrap_admin(&s.state.db).await.unwrap().unwrap();
    let admin = s.login(&user, &pass).await.cookie().unwrap();

    let taken = s
        .post(
            "/account/username",
            &[("username", "ALICE"), ("password", pass.as_str())],
            Some(&admin),
        )
        .await;
    assert_eq!(taken.status, StatusCode::CONFLICT);
    let r = s
        .post(
            "/account/username",
            &[("username", "alice2"), ("password", PASSWORD)],
            Some(&alice),
        )
        .await;
    assert_eq!(
        r.status,
        StatusCode::NOT_FOUND,
        "usernames are fixed for regular users"
    );
    assert!(!s
        .get("/account", Some(&alice))
        .await
        .body
        .contains("Change username"));
    assert!(s
        .get("/account", Some(&admin))
        .await
        .body
        .contains("Change username"));
}

#[tokio::test]
async fn admin_can_add_an_email_that_is_never_shown_and_enables_reset() {
    let s = site().await;
    let (user, _) = auth::bootstrap_admin(&s.state.db).await.unwrap().unwrap();
    // Replace the generated password with a known one, so the form can be used.
    let id = s
        .state
        .db
        .user_for_login(user.clone())
        .await
        .unwrap()
        .unwrap()
        .0
        .id;
    let hash = auth::hash_password(PASSWORD.into()).await.unwrap();
    s.state.db.set_password(id, hash).await.unwrap();
    let admin = s.login_cookie(&user).await;

    // No address yet, so reset does nothing, and the form is offered.
    let before = s.mail_count();
    s.post("/forgot", &[("email", "boss@example.org")], None)
        .await;
    assert_eq!(s.mail_count(), before);
    assert!(s
        .get("/account", Some(&admin))
        .await
        .body
        .contains("Change email"));

    let r = s
        .post(
            "/account/email",
            &[("email", "boss@example.org"), ("password", PASSWORD)],
            Some(&admin),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert!(!r.body.contains("boss@"));
    let link = s.last_link("/email/confirm?token=");
    assert_eq!(s.get(&link, Some(&admin)).await.status, StatusCode::OK);

    // Now reset works for the admin, and nothing displays the address.
    let before = s.mail_count();
    s.post("/forgot", &[("email", "boss@example.org")], None)
        .await;
    assert_eq!(s.mail_count(), before + 1);
    for body in [
        s.get("/account", Some(&admin)).await.body,
        s.get("/admin", Some(&admin)).await.body,
        s.get("/api/me/progress", Some(&admin)).await.body,
    ] {
        assert!(!body.contains("boss@") && !body.contains("example.org"));
    }
}

// ---------------------------------------------------------------- container healthcheck

#[tokio::test]
async fn healthcheck_passes_against_a_running_server_and_fails_without_one() {
    let s = site().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            s.app
                .into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    assert!(
        tokio::task::spawn_blocking(move || crate::healthcheck(addr))
            .await
            .unwrap()
    );
    server.abort();
    let _ = server.await;
    // Nothing is listening any more.
    assert!(
        !tokio::task::spawn_blocking(move || crate::healthcheck(addr))
            .await
            .unwrap()
    );
}

#[test]
fn healthcheck_checks_a_wildcard_address_through_loopback() {
    // Port 9 (discard) is closed; the point is that 0.0.0.0 does not panic or hang.
    assert!(!crate::healthcheck("0.0.0.0:9".parse().unwrap()));
}

// ---------------------------------------------------------------- intro and daily tracks

#[tokio::test]
async fn unreleased_dailies_are_hidden_everywhere() {
    // On 2026-11-25 the first two ILOVEYOU dailies are out; the third is not.
    let s = site_on("2026-11-25").await;
    for path in ["/p/iloveyou-dropped", "/p/iloveyou-attachment"] {
        assert_eq!(s.get(path, None).await.status, StatusCode::OK, "{path}");
    }
    let hidden = "iloveyou-payload";
    assert_eq!(
        s.get(&format!("/p/{hidden}"), None).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        s.get(&format!("/p/{hidden}/files/run-keys.reg"), None)
            .await
            .status,
        StatusCode::NOT_FOUND,
        "its files are hidden too"
    );
    let r = s
        .post_json(
            &format!("/api/p/{hidden}/answer"),
            r#"{"answer":"C:\\WINDOWS\\SYSTEM\\WINFAT32.EXE"}"#,
            None,
        )
        .await;
    assert_eq!(
        r.status,
        StatusCode::NOT_FOUND,
        "its answer cannot be checked early"
    );
    assert!(!r.body.contains("explainer_html"));
    for later in ["peanuts", "second-stage"] {
        assert_eq!(
            s.get(&format!("/p/{later}"), None).await.status,
            StatusCode::NOT_FOUND,
            "{later}"
        );
    }
    let archive = s.get("/archive", None).await.body;
    assert!(archive.contains("iloveyou-dropped") && archive.contains("iloveyou-attachment"));
    assert!(
        !archive.contains(hidden) && !archive.contains("Peanuts"),
        "not listed either"
    );

    // A signed-in solver cannot reach it through the explainer endpoint either.
    s.register_verified("alice", SECRET_EMAIL).await;
    let cookie = s.login_cookie("alice").await;
    let r = s
        .get(&format!("/api/p/{hidden}/explainer"), Some(&cookie))
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn today_is_the_latest_released_daily() {
    let s = site_on("2026-11-24").await;
    let home = s.get("/", None).await.body;
    assert!(home.contains("ILOVEYOU: the source"));
    assert!(home.contains("Daily #1"));
    assert!(
        home.contains(r#"class="badge">Today<"#),
        "dated today, so it is badged"
    );

    let s = site_on("2026-11-26").await;
    assert!(s
        .get("/", None)
        .await
        .body
        .contains("ILOVEYOU: the password stealer"));

    // After the last daily, the latest one stays up but is no longer "Today".
    let s = site_on("2026-12-08").await;
    let home = s.get("/", None).await.body;
    assert!(home.contains("One password, everyone"));
    assert!(
        !home.contains(r#"class="badge">Today<"#),
        "not dated today, so no badge"
    );
}

#[tokio::test]
async fn before_any_daily_the_home_page_goes_to_the_intro_stages() {
    let s = site_on("2020-01-01").await;
    let r = s.get("/", None).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(r.headers[header::LOCATION], "/learn");
    assert_eq!(
        s.get("/p/intro-log", None).await.status,
        StatusCode::OK,
        "intros are always open"
    );
    assert!(!s.get("/archive", None).await.body.contains("ILOVEYOU"));
}

#[tokio::test]
async fn learn_lists_the_intro_stages_in_order() {
    let body = site().await.get("/learn", None).await.body;
    let order = [
        "Who got in?",
        "Encoded command",
        "Hidden in numbers",
        "Follow the chain",
    ];
    let mut last = 0;
    for title in order {
        let at = body
            .find(title)
            .unwrap_or_else(|| panic!("{title} missing"));
        assert!(at > last, "{title} is out of order");
        last = at;
    }
    assert!(
        !body.contains("ILOVEYOU"),
        "dailies are not on the learn page"
    );
    assert!(body.contains("Then: today"));
}

#[tokio::test]
async fn archive_lists_released_dailies_newest_first_with_dates() {
    let body = site().await.get("/archive", None).await.body;
    let newest = body.find("One password, everyone").unwrap();
    let oldest = body.find("ILOVEYOU: the source").unwrap();
    assert!(newest < oldest);
    assert!(body.contains("2026-11-24") && body.contains("2026-12-05"));
    assert!(
        !body.contains("Who got in?"),
        "intro stages are not in the archive"
    );
}

#[tokio::test]
async fn the_pager_stays_in_its_track_and_the_last_intro_leads_to_today() {
    let s = site().await;
    let first = s.get("/p/intro-log", None).await.body;
    assert!(first.contains("Intro 1 of 4"));
    assert!(first.contains("/p/intro-powershell"), "next intro");
    assert!(!first.contains("← "), "no previous before the first");

    let last = s.get("/p/intro-redirects", None).await.body;
    assert!(
        last.contains("Today&#39;s puzzle: One password, everyone")
            || last.contains("Today's puzzle: One password, everyone")
    );
    assert!(last.contains(r#"href="/""#));

    let daily = s.get("/p/iloveyou-attachment", None).await.body;
    assert!(daily.contains("Daily #2") && daily.contains("2026-11-25"));
    assert!(daily.contains("/p/iloveyou-dropped") && daily.contains("/p/iloveyou-payload"));
    assert!(
        daily.contains("intro-banner"),
        "daily pages offer the intro stages"
    );
    assert!(!first.contains("intro-banner"));
}

#[tokio::test]
async fn only_dailies_solved_on_their_release_day_count_for_streaks() {
    let s = site().await;
    s.register_verified("alice", SECRET_EMAIL).await;
    let cookie = s.login_cookie("alice").await;
    let day = necropsy_pack::parse_date("2026-11-27").unwrap(); // peanuts

    s.post_json(
        "/api/p/peanuts/answer",
        r#"{"answer":"sess_9f2c41ab77e0"}"#,
        Some(&cookie),
    )
    .await;
    s.post_json(
        "/api/p/intro-log/answer",
        r#"{"answer":"203.0.113.77"}"#,
        Some(&cookie),
    )
    .await;
    let set_day = |d: i64| {
        let db = s.state.db.clone();
        async move {
            db.run(move |c| {
                c.execute(
                    "UPDATE solves SET solved_at = ?1 WHERE pack_id = 'peanuts'",
                    [d * 86_400 + 100],
                )
            })
            .await
            .unwrap();
        }
    };

    set_day(day).await;
    let p = s.get("/api/me/progress", Some(&cookie)).await.body;
    assert!(
        p.contains(r#""id":"peanuts","day":20784,"ontime":true"#),
        "{p}"
    );
    set_day(day + 3).await;
    let p = s.get("/api/me/progress", Some(&cookie)).await.body;
    assert!(
        p.contains(r#""id":"peanuts","day":20787,"ontime":false"#),
        "a late solve does not count: {p}"
    );
    assert!(
        p.contains(r#""id":"intro-log""#) && p.matches(r#""ontime":true"#).count() == 0,
        "intro stages never count: {p}"
    );
}

// ---------------------------------------------------------------- live puzzles

async fn live_site() -> Site {
    build_full(
        None,
        None,
        &[],
        ALL_RELEASED,
        Some(crate::live::LiveConfig {
            domain: LIVE_DOMAIN.into(),
            host_header: None,
            range: None,
        }),
        crate::captcha::Captcha::Off,
    )
    .await
}

/// A request to an instance's own subdomain.
async fn live_send(
    s: &Site,
    label: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Reply {
    let mut b = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, format!("{label}.{LIVE_DOMAIN}"));
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    s.send(b.body(Body::from(body.to_owned())).unwrap()).await
}

/// Signs in a fresh account and starts its instance of `pack`.
/// Returns the session cookie and the instance's label.
async fn spin_up(s: &Site, user: &str, pack: &str) -> (String, String) {
    s.register_verified(user, &format!("{user}@example.org"))
        .await;
    let cookie = s.login_cookie(user).await;
    let r = s
        .send(
            Request::post(format!("/api/p/{pack}/instance"))
                .header(header::COOKIE, &cookie)
                .header(header::ORIGIN, ORIGIN)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let url = r
        .body
        .split("\"url\":\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let label = url
        .strip_prefix("https://")
        .unwrap()
        .strip_suffix(&format!(".{LIVE_DOMAIN}"))
        .unwrap()
        .to_owned();
    (cookie, label)
}

async fn submit(s: &Site, cookie: &str, pack: &str, answer: &str) -> bool {
    let r = s
        .post_json(
            &format!("/api/p/{pack}/answer"),
            &format!(r#"{{"answer":"{answer}"}}"#),
            Some(cookie),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    r.body.contains(r#""correct":true"#)
}

const PEBBLE: &str = "Pebble/2.4 (Win64; build 5521)";

/// Checks in as the implant and follows the reply to the tasking.
async fn pebble_flag(s: &Site, label: &str) -> String {
    let sync = live_send(
        s,
        label,
        "POST",
        "/api/v2/sync",
        &[
            ("user-agent", PEBBLE),
            ("x-client-id", "7f3a9c1e5b2d4086"),
            ("content-type", "application/x-www-form-urlencoded"),
        ],
        "d=N2YzYTljMWU1YjJkNDA4NnxTQU5EQk9YLTAxfGFuYWx5c3R8V2luZG93cyAxMCAxOTA0NQ%3D%3D",
    )
    .await;
    assert_eq!(sync.status, StatusCode::OK, "{}", sync.body);
    let path = sync
        .body
        .split("\"t\":\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let task = live_send(
        s,
        label,
        "GET",
        path,
        &[("user-agent", PEBBLE), ("x-client-id", "7f3a9c1e5b2d4086")],
        "",
    )
    .await;
    assert_eq!(task.status, StatusCode::OK, "{}", task.body);
    let key = task.body.split("\"key\":\"").nth(1).unwrap();
    key.split('"').next().unwrap().to_owned()
}

#[tokio::test]
async fn live_puzzles_are_hidden_without_a_live_domain() {
    let s = site().await;
    assert_eq!(
        s.get("/p/c2-gate", None).await.status,
        StatusCode::NOT_FOUND
    );
    let s = live_site().await;
    assert_eq!(s.get("/p/c2-gate", None).await.status, StatusCode::OK);
}

#[tokio::test]
async fn an_instance_needs_an_account() {
    let s = live_site().await;
    let r = s
        .send(
            Request::post("/api/p/c2-gate/instance")
                .header(header::ORIGIN, ORIGIN)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    // And a flag means nothing to someone who has no instance.
    assert!(!submit(&s, "x=y", "c2-gate", "pebble{00000000000000000000}").await);
}

#[tokio::test]
async fn c2_gate_can_be_solved_and_flags_are_per_player() {
    let s = live_site().await;
    let (alice, a) = spin_up(&s, "alice", "c2-gate").await;
    let (bob, b) = spin_up(&s, "bob", "c2-gate").await;

    // A browser, a scanner, or a wrong build gets a dull 404.
    for ua in [
        "Mozilla/5.0",
        "curl/8.5.0",
        "Pebble/2.3 (Win64; build 5521)",
    ] {
        let r = live_send(&s, &a, "POST", "/api/v2/sync", &[("user-agent", ua)], "d=x").await;
        assert_eq!(r.status, StatusCode::NOT_FOUND);
        assert!(r.body.contains("nginx"), "{}", r.body);
    }
    // The tasking is not reachable without checking in.
    let r = live_send(
        &s,
        &a,
        "GET",
        "/api/v2/task/anything",
        &[("user-agent", PEBBLE)],
        "",
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // The right implant with a bad frame is told so.
    let r = live_send(
        &s,
        &a,
        "POST",
        "/api/v2/sync",
        &[
            ("user-agent", PEBBLE),
            ("x-client-id", "nothex"),
            ("content-type", "application/x-www-form-urlencoded"),
        ],
        "d=x",
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    let fa = pebble_flag(&s, &a).await;
    let fb = pebble_flag(&s, &b).await;
    assert!(fa.starts_with("pebble{") && fb.starts_with("pebble{"));
    assert_ne!(fa, fb, "every player gets their own flag");

    assert!(
        !submit(&s, &alice, "c2-gate", &fb).await,
        "bob's flag is not alice's"
    );
    assert!(submit(&s, &alice, "c2-gate", &fa).await);
    assert!(submit(&s, &bob, "c2-gate", &fb.to_uppercase()).await);
    // Solving is recorded like any other puzzle.
    let p = s.get("/api/me/progress", Some(&alice)).await.body;
    assert!(p.contains(r#""id":"c2-gate""#), "{p}");
}

#[tokio::test]
async fn kit_left_open_leaks_in_order() {
    let s = live_site().await;
    let (cookie, l) = spin_up(&s, "carol", "kit-left-open").await;

    let robots = live_send(&s, &l, "GET", "/robots.txt", &[], "").await;
    assert!(robots.body.contains("/panel/"));
    assert_eq!(
        live_send(&s, &l, "GET", "/panel/", &[], "").await.status,
        StatusCode::UNAUTHORIZED
    );
    let cfg = live_send(&s, &l, "GET", "/backup/config.php.bak", &[], "").await;
    let pw = cfg
        .body
        .split("$panel_pass = '")
        .nth(1)
        .unwrap()
        .split('\'')
        .next()
        .unwrap();
    assert_eq!(pw.len(), 12);

    use base64::Engine;
    let auth = |u: &str| {
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("{u}:{pw}"))
        )
    };
    let bad = live_send(
        &s,
        &l,
        "GET",
        "/panel/",
        &[("authorization", &auth("root"))],
        "",
    )
    .await;
    assert_eq!(bad.status, StatusCode::UNAUTHORIZED);
    let ok = live_send(
        &s,
        &l,
        "GET",
        "/panel/",
        &[("authorization", &auth("admin"))],
        "",
    )
    .await;
    assert_eq!(ok.status, StatusCode::OK);
    let flag = ok
        .body
        .split("<code>")
        .nth(1)
        .unwrap()
        .split("</code>")
        .next()
        .unwrap();
    assert!(submit(&s, &cookie, "kit-left-open", flag).await);
}

#[tokio::test]
async fn instances_expire_stop_and_are_capped() {
    let s = live_site().await;
    let (cookie, l) = spin_up(&s, "dave", "kit-left-open").await;
    assert_eq!(
        live_send(&s, &l, "GET", "/robots.txt", &[], "")
            .await
            .status,
        StatusCode::OK
    );

    // Unknown labels and names under the domain that are not instances.
    assert_eq!(
        live_send(&s, "nosuchlabel", "GET", "/", &[], "")
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        live_send(&s, "a.b", "GET", "/", &[], "").await.status,
        StatusCode::NOT_FOUND
    );

    // Starting again returns the running one.
    let again = s
        .send(
            Request::post("/api/p/kit-left-open/instance")
                .header(header::COOKIE, &cookie)
                .header(header::ORIGIN, ORIGIN)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert!(again.body.contains(&l), "{}", again.body);

    // Stopping ends it at once.
    let stop = s
        .send(
            Request::post("/api/p/kit-left-open/instance/stop")
                .header(header::COOKIE, &cookie)
                .header(header::ORIGIN, ORIGIN)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(stop.status, StatusCode::OK);
    assert_eq!(
        live_send(&s, &l, "GET", "/robots.txt", &[], "")
            .await
            .status,
        StatusCode::GONE
    );

    // An instance past its time says so, and its flag still counts for a while.
    let (_, l2) = spin_up(&s, "erin", "c2-gate").await;
    let flag = pebble_flag(&s, &l2).await;
    let l3 = l2.clone();
    s.state
        .db
        .run(move |c| {
            c.execute(
                "UPDATE live_instances SET expires_at = 1 WHERE label = ?1",
                [l3],
            )
        })
        .await
        .unwrap();
    assert_eq!(
        live_send(&s, &l2, "GET", "/", &[], "").await.status,
        StatusCode::GONE
    );
    let erin = s.login_cookie("erin").await;
    assert!(submit(&s, &erin, "c2-gate", &flag).await);

    // At most three running at once: frank has one real instance and two
    // more (other puzzles) stand in for the rest.
    let (frank, fl) = spin_up(&s, "frank", "c2-gate").await;
    s.state
        .db
        .run(move |c| {
            for n in 0..2 {
                c.execute(
                    "INSERT INTO live_instances (label, user_id, pack_id, seed, created_at, expires_at)
                     SELECT ?2, user_id, ?3, 's', 0, 9999999999 FROM live_instances WHERE label = ?1",
                    rusqlite::params![fl, format!("extra{n}"), format!("other-{n}")],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();
    let r = s
        .send(
            Request::post("/api/p/kit-left-open/instance")
                .header(header::COOKIE, &frank)
                .header(header::ORIGIN, ORIGIN)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS, "{}", r.body);
}

#[tokio::test]
async fn live_hosts_never_reach_the_main_site_and_cannot_post_to_it() {
    let s = live_site().await;
    let (_, l) = spin_up(&s, "gina", "kit-left-open").await;

    // The main site's own pages do not exist on an instance's host.
    for path in ["/login", "/account", "/admin", "/api/me/progress"] {
        let r = live_send(&s, &l, "GET", path, &[], "").await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{path}");
        assert!(!r.body.to_lowercase().contains("password"), "{path}");
    }
    // Instances are not framed or sniffed.
    let r = live_send(&s, &l, "GET", "/robots.txt", &[], "").await;
    assert_eq!(r.headers[header::X_FRAME_OPTIONS], "DENY");
    assert_eq!(r.headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");

    // A page on an instance cannot make a browser post to the main site.
    let r = s
        .send(
            Request::post("/logout")
                .header(header::ORIGIN, format!("https://{l}.{LIVE_DOMAIN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn session_cookie_is_host_only_over_https() {
    let s = site().await;
    s.register_verified("hank", "hank@example.org").await;
    let r = s.login("hank", PASSWORD).await;
    let set = r.headers.get(header::SET_COOKIE).unwrap().to_str().unwrap();
    assert!(set.starts_with("__Host-necropsy_session="), "{set}");
    assert!(set.contains("Secure") && set.contains("Path=/") && !set.contains("Domain"));
}

// ---------------------------------------------------------------- captcha

async fn captcha_site() -> Site {
    build_full(
        None,
        None,
        &[],
        ALL_RELEASED,
        Some(crate::live::LiveConfig {
            domain: LIVE_DOMAIN.into(),
            host_header: None,
            range: None,
        }),
        crate::captcha::Captcha::Fake,
    )
    .await
}

#[tokio::test]
async fn registration_needs_the_captcha_when_one_is_set_up() {
    let s = captcha_site().await;
    let page = s.get("/register", None).await;
    assert!(page.body.contains("cf-turnstile") && page.body.contains("test-site-key"));
    let fields = |token: &'static str| {
        vec![
            ("username", "ivy"),
            ("email", "ivy@example.org"),
            ("password", PASSWORD),
            ("cf-turnstile-response", token),
        ]
    };
    let r = s.post("/register", &fields(""), None).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.body);
    assert!(r.body.contains("captcha"));
    let r = s.post("/register", &fields("wrong"), None).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(
        s.mail.lock().unwrap().is_empty(),
        "no email for a failed captcha"
    );
    let r = s.post("/register", &fields("pass"), None).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
}

#[tokio::test]
async fn no_captcha_widget_when_none_is_set_up() {
    let s = site().await;
    assert!(!s.get("/register", None).await.body.contains("cf-turnstile"));
}

#[tokio::test]
async fn starting_an_instance_needs_the_captcha_but_looking_does_not() {
    let s = captcha_site().await;
    s.register_verified_with_captcha("jack").await;
    let cookie = s.login_cookie("jack").await;
    let start = |token: Option<&'static str>| {
        let mut b = Request::post("/api/p/kit-left-open/instance")
            .header(header::COOKIE, cookie.clone())
            .header(header::ORIGIN, ORIGIN);
        if let Some(t) = token {
            b = b.header("x-turnstile-token", t);
        }
        b.body(Body::empty()).unwrap()
    };
    assert_eq!(s.send(start(None)).await.status, StatusCode::FORBIDDEN);
    assert_eq!(
        s.send(start(Some("wrong"))).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(s.send(start(Some("pass"))).await.status, StatusCode::OK);
    // The running one is returned without asking again.
    assert_eq!(s.send(start(None)).await.status, StatusCode::OK);
    let page = s.get("/p/kit-left-open", Some(&cookie)).await.body;
    assert!(page.contains(r#"data-sitekey="test-site-key""#), "{page}");
}

impl Site {
    async fn register_verified_with_captcha(&self, username: &str) {
        let r = self
            .post(
                "/register",
                &[
                    ("username", username),
                    ("email", &format!("{username}@example.org")),
                    ("password", PASSWORD),
                    ("cf-turnstile-response", "pass"),
                ],
                None,
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.body);
        let link = self.last_link("/verify?token=");
        assert_eq!(self.get(&link, None).await.status, StatusCode::OK);
    }
}

// ------------------------------------------------------- container puzzles

/// What the stub range VM was asked, in order: "POST /v1/instances {...}", ...
type Calls = Arc<Mutex<Vec<String>>>;

const RANGE_TOKEN: &str = "range-token-0123456789abcdef";

struct Range {
    url: String,
    calls: Calls,
    /// Make the stub refuse to start instances, with this status.
    refuse: Arc<Mutex<Option<StatusCode>>>,
    /// Labs the stub "has", by id. Lab traffic to any other id is a 502.
    labs: Arc<Mutex<Vec<String>>>,
}

/// A stand-in for the range VM, on a local port. It answers the control API
/// (recording each call) and, for a request carrying `X-Necropsy-Instance`, plays
/// the lab: it echoes the method, path, body and a few headers.
async fn stub_range() -> Range {
    use axum::extract::{Request as AxRequest, State};

    #[derive(Clone)]
    struct St {
        calls: Calls,
        refuse: Arc<Mutex<Option<StatusCode>>>,
        labs: Arc<Mutex<Vec<String>>>,
    }
    let st = St {
        calls: Arc::default(),
        refuse: Arc::default(),
        labs: Arc::default(),
    };
    let (calls, refuse, labs) = (st.calls.clone(), st.refuse.clone(), st.labs.clone());

    async fn handler(State(st): State<St>, req: AxRequest) -> axum::response::Response {
        use axum::response::IntoResponse;
        let (parts, body) = req.into_parts();
        let body = String::from_utf8_lossy(&axum::body::to_bytes(body, 1 << 20).await.unwrap())
            .into_owned();
        let header = |n: &str| {
            parts
                .headers
                .get(n)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        if parts.uri.path() == "/healthz" {
            return "ok".into_response();
        }
        // Every call carries this site's token; nothing is served without it.
        if header("x-necropsy-token").as_deref() != Some(RANGE_TOKEN) {
            return (StatusCode::UNAUTHORIZED, "no").into_response();
        }
        // Lab traffic.
        if let Some(id) = header("x-necropsy-instance") {
            st.calls
                .lock()
                .unwrap()
                .push(format!("LAB {id} {} {}", parts.method, parts.uri));
            if !st.labs.lock().unwrap().contains(&id) {
                return (StatusCode::NOT_FOUND, "no such instance").into_response();
            }
            let mut names: Vec<&str> = parts
                .headers
                .keys()
                .map(|k| k.as_str())
                .filter(|k| k.starts_with("x-"))
                .collect();
            names.sort();
            return (
                [("x-from-lab", "yes"), ("content-type", "text/plain")],
                format!(
                    "{} {} body={} auth={} headers={}",
                    parts.method,
                    parts.uri,
                    body,
                    header("authorization").unwrap_or_else(|| "-".into()),
                    names.join(",")
                ),
            )
                .into_response();
        }
        // Control API.
        let path = parts.uri.path().to_owned();
        st.calls
            .lock()
            .unwrap()
            .push(format!("{} {path} {body}", parts.method));
        if parts.method == axum::http::Method::POST && path == "/v1/instances" {
            if let Some(code) = *st.refuse.lock().unwrap() {
                // What the real range says when a lab has not been shipped to it.
                let why = if code == StatusCode::BAD_REQUEST {
                    "that lab is not on the allowlist"
                } else {
                    "no"
                };
                return (code, why).into_response();
            }
            let v: serde_json::Value = serde_json::from_str(&body).unwrap();
            st.labs
                .lock()
                .unwrap()
                .push(v["id"].as_str().unwrap().to_owned());
            return (StatusCode::CREATED, "created").into_response();
        }
        if parts.method == axum::http::Method::DELETE {
            let id = path.rsplit('/').next().unwrap().to_owned();
            st.labs.lock().unwrap().retain(|l| *l != id);
        }
        "ok".into_response()
    }

    let app = Router::new().fallback(handler).with_state(st);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Range {
        url,
        calls,
        refuse,
        labs,
    }
}

async fn container_site(range: &Range) -> Site {
    build_full(
        None,
        None,
        &[],
        ALL_RELEASED,
        Some(crate::live::LiveConfig {
            domain: LIVE_DOMAIN.into(),
            host_header: None,
            range: Some(crate::live::Range::new(&range.url, RANGE_TOKEN)),
        }),
        crate::captcha::Captcha::Off,
    )
    .await
}

fn created_flag(range: &Range) -> String {
    let calls = range.calls.lock().unwrap();
    let create = calls
        .iter()
        .find(|c| c.starts_with("POST /v1/instances "))
        .unwrap();
    let json = create.trim_start_matches("POST /v1/instances ");
    let v: serde_json::Value = serde_json::from_str(json).unwrap();
    v["env"]["NECROPSY_FLAG"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn container_puzzles_are_hidden_without_a_range() {
    // Live is on, but no range VM is configured.
    let s = live_site().await;
    assert_eq!(
        s.get("/p/webshell-lab", None).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(s.get("/p/c2-gate", None).await.status, StatusCode::OK);
    let range = stub_range().await;
    let s = container_site(&range).await;
    assert_eq!(s.get("/p/webshell-lab", None).await.status, StatusCode::OK);
}

#[tokio::test]
async fn a_container_instance_is_started_proxied_and_stopped() {
    let range = stub_range().await;
    let s = container_site(&range).await;
    let (cookie, l) = spin_up(&s, "kim", "webshell-lab").await;

    // The range was asked for the right lab, id, time and flag.
    {
        let calls = range.calls.lock().unwrap();
        let c = &calls[0];
        assert!(c.contains(r#""image":"webshell-lab""#), "{c}");
        assert!(c.contains(&format!(r#""id":"{l}""#)), "{c}");
        assert!(c.contains(r#""ttl_secs":2700"#), "{c}");
    }
    let flag = created_flag(&range);
    assert!(flag.starts_with("shell{") && flag.ends_with('}'), "{flag}");

    // Requests reach the lab, with their method, path, headers and body, and its
    // answer comes back with our safety headers added.
    let r = live_send(&s, &l, "GET", "/a/b?x=1", &[("x-auth", "k")], "").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert!(
        r.body.starts_with("GET /a/b?x=1 body= auth=-"),
        "{}",
        r.body
    );
    assert!(
        r.body
            .contains("headers=x-auth,x-necropsy-instance,x-necropsy-token"),
        "only ours were added: {}",
        r.body
    );
    assert_eq!(r.headers["x-from-lab"], "yes");
    assert_eq!(r.headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    let r = live_send(&s, &l, "POST", "/p", &[], "q=id").await;
    assert!(r.body.starts_with("POST /p body=q=id"), "{}", r.body);
    // The player's own Authorization header reaches the lab: ours is a different one.
    let r = live_send(
        &s,
        &l,
        "GET",
        "/",
        &[("authorization", "Basic cGxheWVy")],
        "",
    )
    .await;
    assert!(r.body.contains("auth=Basic cGxheWVy"), "{}", r.body);

    // The player sees these in their log.
    let st = s
        .get("/api/p/webshell-lab/instance", Some(&cookie))
        .await
        .body;
    assert!(
        st.contains("/a/b?x=1") && st.contains(r#""method":"POST""#),
        "{st}"
    );

    // The flag is accepted from the right player.
    assert!(submit(&s, &cookie, "webshell-lab", &flag).await);

    // Stopping tells the range, and the instance is gone.
    let stop = s
        .send(
            Request::post("/api/p/webshell-lab/instance/stop")
                .header(header::COOKIE, &cookie)
                .header(header::ORIGIN, ORIGIN)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(stop.status, StatusCode::OK);
    assert!(range
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|c| *c == format!("DELETE /v1/instances/{l} ")));
    assert_eq!(
        live_send(&s, &l, "GET", "/", &[], "").await.status,
        StatusCode::GONE
    );
}

#[tokio::test]
async fn a_player_cannot_choose_the_lab_or_borrow_the_token() {
    let range = stub_range().await;
    let s = container_site(&range).await;
    let (_, mine) = spin_up(&s, "pat", "webshell-lab").await;
    let (_, theirs) = spin_up(&s, "quinn", "webshell-lab").await;
    // Pat asks, on their own instance's host, for someone else's lab and sends a token.
    let r = live_send(
        &s,
        &mine,
        "GET",
        "/",
        &[
            ("x-necropsy-instance", &theirs),
            ("x-necropsy-token", "stolen-0123456789abcdef"),
        ],
        "",
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert!(!r.body.contains("stolen"), "{}", r.body);
    let calls = range.calls.lock().unwrap();
    assert!(
        calls
            .iter()
            .any(|c| c.starts_with(&format!("LAB {mine} GET"))),
        "{calls:?}"
    );
    assert!(
        !calls
            .iter()
            .any(|c| c.starts_with(&format!("LAB {theirs} GET"))),
        "the player's header did not pick another lab: {calls:?}"
    );
}

#[tokio::test]
async fn a_full_or_broken_range_fails_cleanly() {
    let range = stub_range().await;
    let s = container_site(&range).await;
    s.register_verified("lou", "lou@example.org").await;
    let cookie = s.login_cookie("lou").await;
    let start = || {
        Request::post("/api/p/webshell-lab/instance")
            .header(header::COOKIE, cookie.clone())
            .header(header::ORIGIN, ORIGIN)
            .body(Body::empty())
            .unwrap()
    };

    *range.refuse.lock().unwrap() = Some(StatusCode::TOO_MANY_REQUESTS);
    let r = s.send(start()).await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(r.body.contains("full"), "{}", r.body);
    *range.refuse.lock().unwrap() = Some(StatusCode::BAD_GATEWAY);
    assert_eq!(
        s.send(start()).await.status,
        StatusCode::SERVICE_UNAVAILABLE
    );

    // Neither left an instance behind, so there is nothing to count against the cap.
    let st = s.get("/api/p/webshell-lab/instance", Some(&cookie)).await;
    assert!(st.body.contains(r#""active":false"#), "{}", st.body);

    // And once the range recovers, starting works.
    *range.refuse.lock().unwrap() = None;
    assert_eq!(s.send(start()).await.status, StatusCode::OK);
}

#[tokio::test]
async fn an_unreachable_range_is_a_clean_error_and_use_extends_the_instance() {
    let range = stub_range().await;
    let s = container_site(&range).await;
    let (_, l) = spin_up(&s, "max", "webshell-lab").await;

    // Close to its end: a request gives it a fresh term, here and at the range.
    let l2 = l.clone();
    let soon = crate::db::now() + 60;
    s.state
        .db
        .run(move |c| {
            c.execute(
                "UPDATE live_instances SET expires_at = ?2 WHERE label = ?1",
                rusqlite::params![l2, soon],
            )
        })
        .await
        .unwrap();
    assert_eq!(
        live_send(&s, &l, "GET", "/", &[], "").await.status,
        StatusCode::OK
    );
    for _ in 0..50 {
        if range
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.contains("/touch"))
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let touched = range
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|c| c.starts_with(&format!("POST /v1/instances/{l}/touch")));
    assert!(touched, "{:?}", range.calls.lock().unwrap());
    let inst = s.state.db.live_by_label(l.clone()).await.unwrap().unwrap();
    assert!(
        inst.expires_at > crate::db::now() + 2000,
        "extended to a fresh term"
    );

    // A lab the range no longer has: its 404 is passed on, not turned into a hang.
    range.labs.lock().unwrap().clear();
    assert_eq!(
        live_send(&s, &l, "GET", "/", &[], "").await.status,
        StatusCode::NOT_FOUND
    );

    // A range that cannot be reached is a 502.
    let dead = {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        format!("http://{}", l.local_addr().unwrap())
    };
    let s2 = build_full(
        None,
        None,
        &[],
        ALL_RELEASED,
        Some(crate::live::LiveConfig {
            domain: LIVE_DOMAIN.into(),
            host_header: None,
            range: Some(crate::live::Range::new(&dead, RANGE_TOKEN)),
        }),
        crate::captcha::Captcha::Off,
    )
    .await;
    s2.register_verified("zed", "zed@example.org").await;
    let cookie = s2.login_cookie("zed").await;
    let r = s2
        .send(
            Request::post("/api/p/webshell-lab/instance")
                .header(header::COOKIE, &cookie)
                .header(header::ORIGIN, ORIGIN)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE, "{}", r.body);
    let st = s2.get("/api/p/webshell-lab/instance", Some(&cookie)).await;
    assert!(
        st.body.contains(r#""active":false"#),
        "no instance is left behind: {}",
        st.body
    );
}

#[tokio::test]
async fn a_wrong_range_token_is_refused_by_the_range() {
    let range = stub_range().await;
    let s = build_full(
        None,
        None,
        &[],
        ALL_RELEASED,
        Some(crate::live::LiveConfig {
            domain: LIVE_DOMAIN.into(),
            host_header: None,
            range: Some(crate::live::Range::new(
                &range.url,
                "wrong-token-0123456789abcdef",
            )),
        }),
        crate::captcha::Captcha::Off,
    )
    .await;
    s.register_verified("tia", "tia@example.org").await;
    let cookie = s.login_cookie("tia").await;
    let r = s
        .send(
            Request::post("/api/p/webshell-lab/instance")
                .header(header::COOKIE, &cookie)
                .header(header::ORIGIN, ORIGIN)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(range.labs.lock().unwrap().is_empty());
}

// ------------------------------------------------------------- the live track

/// A live-enabled site whose clock says it is long before any daily was released.
async fn live_site_on(today: &str) -> Site {
    build_full(
        None,
        None,
        &[],
        today,
        Some(crate::live::LiveConfig {
            domain: LIVE_DOMAIN.into(),
            host_header: None,
            range: None,
        }),
        crate::captcha::Captcha::Off,
    )
    .await
}

#[tokio::test]
async fn live_puzzles_are_always_available_and_not_part_of_the_dailies() {
    // Before the first daily, and so before any date a live puzzle used to carry.
    let s = live_site_on("2020-01-01").await;
    assert_eq!(s.get("/p/c2-gate", None).await.status, StatusCode::OK);
    assert_eq!(s.get("/p/kit-left-open", None).await.status, StatusCode::OK);
    // Dailies are still hidden then.
    assert_eq!(
        s.get("/p/peanuts", None).await.status,
        StatusCode::NOT_FOUND
    );

    let page = s.get("/p/c2-gate", None).await.body;
    assert!(page.contains("Live 1 of 2"), "{page}");
    assert!(
        page.contains("Kit left open"),
        "the pager leads to the next live puzzle"
    );
    assert!(
        !page.contains("Daily #") && !page.contains("Intro 1 of"),
        "it is not a daily or an intro stage"
    );

    let list = s.get("/live", None).await;
    assert_eq!(list.status, StatusCode::OK);
    assert!(list.body.contains("Answer the door") && list.body.contains("Kit left open"));
    assert!(list.body.contains("href=\"/login\""), "it says to log in");
    assert!(
        !list.body.contains("Back inside the shell"),
        "a lab needs the range, so it is not loaded"
    );
    // They are not in the archive or the daily rotation, and the nav links to them.
    assert!(!s
        .get("/archive", None)
        .await
        .body
        .contains("Answer the door"));
    assert!(s.get("/learn", None).await.body.contains("href=\"/live\""));
    // The progress API lists them as playable.
    assert!(s
        .get("/api/me/progress", None)
        .await
        .body
        .contains("c2-gate"));
}

#[tokio::test]
async fn without_live_puzzles_there_is_no_live_page_or_link() {
    let s = site().await;
    assert!(!s.get("/learn", None).await.body.contains("href=\"/live\""));
    let list = s.get("/live", None).await;
    assert!(!list.body.contains("Answer the door"));
    assert_eq!(
        s.get("/p/c2-gate", None).await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn the_live_track_is_ordered_after_the_dailies() {
    let packs = necropsy_pack::load_all(std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../packs"
    )))
    .unwrap();
    let tracks: Vec<_> = packs.iter().map(|p| p.meta.track).collect();
    let last_daily = tracks
        .iter()
        .rposition(|t| *t == necropsy_pack::Track::Daily)
        .unwrap();
    let first_live = tracks
        .iter()
        .position(|t| *t == necropsy_pack::Track::Live)
        .unwrap();
    assert!(first_live > last_daily, "live puzzles come last");
    assert!(packs
        .iter()
        .filter(|p| p.meta.track == necropsy_pack::Track::Live)
        .all(|p| p.meta.date.is_none()));
}

// ------------------------------------------------------- admin: start any live puzzle

fn admin_post(path: &str, cookie: Option<&str>) -> Request<Body> {
    let mut b = Request::post(path).header(header::ORIGIN, ORIGIN);
    if let Some(c) = cookie {
        b = b.header(header::COOKIE, c);
    }
    b.body(Body::empty()).unwrap()
}

#[tokio::test]
async fn only_the_admin_can_start_a_live_puzzle_from_the_admin_page() {
    let s = live_site().await;
    s.register_verified("not-admin", "na@example.org").await;
    let user = s.login_cookie("not-admin").await;
    let admin = s.admin_cookie().await;

    assert_eq!(
        s.send(admin_post("/admin/live/c2-gate/start", None))
            .await
            .status,
        StatusCode::SEE_OTHER,
        "to the login"
    );
    assert_eq!(
        s.send(admin_post("/admin/live/c2-gate/start", Some(&user)))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        s.send(admin_post("/admin/live/c2-gate/stop", Some(&user)))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        s.get("/admin", Some(&user)).await.status,
        StatusCode::NOT_FOUND
    );
    // Nothing was started by those.
    assert!(!s
        .get("/admin", Some(&admin))
        .await
        .body
        .contains("min left"));
    // A puzzle that is not live, or does not exist, cannot be started.
    assert_eq!(
        s.send(admin_post("/admin/live/intro-log/start", Some(&admin)))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        s.send(admin_post("/admin/live/nope/start", Some(&admin)))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn the_admin_starts_tries_and_stops_any_live_puzzle() {
    // With a captcha set up, which the admin does not need.
    let s = captcha_site().await;
    let admin = s.admin_cookie().await;

    let page = s.get("/admin", Some(&admin)).await.body;
    assert!(page.contains("Live puzzles"), "{page}");
    for title in ["Answer the door", "Kit left open"] {
        assert!(page.contains(title), "{title} is listed");
    }
    assert!(page.contains("/admin/live/c2-gate/start"), "{page}");
    assert!(!page.contains("min left"), "nothing is running yet");

    let r = s
        .send(admin_post("/admin/live/c2-gate/start", Some(&admin)))
        .await;
    assert_eq!(
        r.status,
        StatusCode::SEE_OTHER,
        "no captcha was asked of the admin: {}",
        r.body
    );

    // The page now shows the link and the flag.
    let page = s.get("/admin", Some(&admin)).await.body;
    assert!(page.contains("min left"), "{page}");
    let url = page
        .split("href=\"")
        .skip(1)
        .filter_map(|r| r.split('"').next())
        .find(|h| h.ends_with(LIVE_DOMAIN))
        .unwrap_or_else(|| panic!("the instance's link is shown: {page}"));
    let label = url
        .split("://")
        .nth(1)
        .unwrap()
        .strip_suffix(&format!(".{LIVE_DOMAIN}"))
        .unwrap()
        .to_owned();
    let shown_flag = page
        .split("flag </span><code>")
        .nth(1)
        .and_then(|r| r.split("</code>").next())
        .unwrap_or_else(|| panic!("the flag is shown: {page}"))
        .to_owned();
    assert!(shown_flag.starts_with("pebble{"), "{shown_flag}");

    // The instance really serves, and the flag shown is the one it hands out.
    assert_eq!(live_flag_of(&s, &label).await, shown_flag);

    // Starting again changes nothing.
    s.send(admin_post("/admin/live/c2-gate/start", Some(&admin)))
        .await;
    assert!(s.get("/admin", Some(&admin)).await.body.contains(&label));

    // Another puzzle, at the same time, with no limit of three.
    s.send(admin_post("/admin/live/kit-left-open/start", Some(&admin)))
        .await;
    let page = s.get("/admin", Some(&admin)).await.body;
    assert_eq!(page.matches("min left").count(), 2, "{page}");

    // Stopping ends it at once, and the page offers to start it again.
    let r = s
        .send(admin_post("/admin/live/c2-gate/stop", Some(&admin)))
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(
        live_send(&s, &label, "GET", "/", &[], "").await.status,
        StatusCode::GONE
    );
    let page = s.get("/admin", Some(&admin)).await.body;
    assert!(
        page.contains("/admin/live/c2-gate/start") && page.matches("min left").count() == 1,
        "{page}"
    );
}

/// What the Pebble implant is told is the key, on an instance, by checking in as it.
async fn live_flag_of(s: &Site, label: &str) -> String {
    pebble_flag(s, label).await
}

#[tokio::test]
async fn the_admin_can_start_a_lab_and_it_reaches_the_range() {
    let range = stub_range().await;
    let s = container_site(&range).await;
    let admin = s.admin_cookie().await;
    let page = s.get("/admin", Some(&admin)).await.body;
    assert!(
        page.contains("a lab on the range") && page.contains("/admin/live/webshell-lab/start"),
        "{page}"
    );

    let r = s
        .send(admin_post("/admin/live/webshell-lab/start", Some(&admin)))
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    let calls = range.calls.lock().unwrap().clone();
    assert!(
        calls
            .iter()
            .any(|c| c.starts_with("POST /v1/instances ") && c.contains("webshell-lab")),
        "{calls:?}"
    );
    let flag = created_flag(&range);
    assert!(
        s.get("/admin", Some(&admin)).await.body.contains(&flag),
        "the flag is shown to the admin"
    );

    let r = s
        .send(admin_post("/admin/live/webshell-lab/stop", Some(&admin)))
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert!(range
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|c| c.starts_with("DELETE /v1/instances/")));
    assert!(!s
        .get("/admin", Some(&admin))
        .await
        .body
        .contains("min left"));
}

#[tokio::test]
async fn a_range_that_refuses_is_reported_to_the_admin_and_leaves_nothing_behind() {
    let range = stub_range().await;
    let s = container_site(&range).await;
    let admin = s.admin_cookie().await;
    *range.refuse.lock().unwrap() = Some(StatusCode::TOO_MANY_REQUESTS);
    let r = s
        .send(admin_post("/admin/live/webshell-lab/start", Some(&admin)))
        .await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(r.body.contains("full"), "{}", r.body);
    assert!(!s
        .get("/admin", Some(&admin))
        .await
        .body
        .contains("min left"));
}

#[tokio::test]
async fn the_admin_page_says_when_live_puzzles_are_off() {
    let s = site().await;
    let admin = s.admin_cookie().await;
    let page = s.get("/admin", Some(&admin)).await.body;
    assert!(page.contains("Live puzzles are off"), "{page}");
    assert_eq!(
        s.send(admin_post("/admin/live/c2-gate/start", Some(&admin)))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

// ---------------------------------------------------------------- request logging

thread_local! {
    static CAPTURE: std::cell::RefCell<Option<Vec<u8>>> = const { std::cell::RefCell::new(None) };
}

/// A writer for the one global test subscriber: it keeps what is logged on the
/// current thread while a test is capturing, and drops the rest. A global
/// subscriber, not one per test, because `tracing` remembers whether each log
/// site is enabled, and a site first hit with no subscriber would stay silent.
#[derive(Clone, Copy)]
struct Routed;

impl std::io::Write for Routed {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        CAPTURE.with(|c| {
            if let Some(v) = c.borrow_mut().as_mut() {
                v.extend_from_slice(b);
            }
        });
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Routed {
    type Writer = Routed;
    fn make_writer(&'a self) -> Routed {
        Routed
    }
}

/// Runs `f` and returns what it logged on this thread.
fn capture_logs(f: impl FnOnce()) -> String {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        let _ = tracing::subscriber::set_global_default(
            tracing_subscriber::fmt()
                .with_writer(Routed)
                .with_ansi(false)
                .finish(),
        );
    });
    CAPTURE.with(|c| *c.borrow_mut() = Some(Vec::new()));
    f();
    let out = CAPTURE.with(|c| c.borrow_mut().take().unwrap_or_default());
    String::from_utf8_lossy(&out).into_owned()
}

#[test]
fn requests_are_logged_only_when_asked_and_never_with_the_query_string() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    // Off by default: a handful of requests leave no trace.
    let quiet_log = capture_logs(|| {
        rt.block_on(async {
            let quiet = site().await;
            for path in [
                "/",
                "/learn",
                "/p/intro-log",
                "/nope",
                "/healthz?token=secret-one",
            ] {
                quiet.get(path, None).await;
            }
        })
    });
    assert!(
        !quiet_log.contains("GET "),
        "nothing is logged unless asked for: {quiet_log}"
    );

    // On: method, path and status, and never the query string.
    let log = capture_logs(|| {
        rt.block_on(async {
            let loud = build_logging(
                None,
                None,
                &[],
                ALL_RELEASED,
                None,
                crate::captcha::Captcha::Off,
                true,
            )
            .await;
            for uri in ["/learn", "/verify?token=secret-two"] {
                loud.get(uri, None).await;
            }
        })
    });
    assert!(log.contains("GET /learn 200"), "{log}");
    assert!(log.contains("GET /verify "), "{log}");
    assert!(
        !log.contains("secret"),
        "the query string never reaches the log: {log}"
    );
}

// ------------------------------------------------------------ reaching the range

#[tokio::test]
async fn the_range_check_says_what_is_wrong() {
    let range = stub_range().await;
    let ok = crate::live::Range::new(&range.url, RANGE_TOKEN);
    assert_eq!(ok.check().await, Ok(()));

    let wrong = crate::live::Range::new(&range.url, "wrong-token-0123456789abcdef");
    let why = wrong.check().await.unwrap_err();
    assert!(why.contains("rejects this site's token"), "{why}");

    // Nothing listening: the real cause, not just "error sending request".
    let dead = {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        format!("http://{}", l.local_addr().unwrap())
    };
    let why = crate::live::Range::new(&dead, RANGE_TOKEN)
        .check()
        .await
        .unwrap_err();
    assert!(why.contains(&format!("cannot reach {dead}")), "{why}");
    assert!(
        why.to_lowercase().contains("refused"),
        "the cause is named: {why}"
    );
}

#[tokio::test]
async fn the_admin_page_shows_whether_the_range_is_usable() {
    let range = stub_range().await;
    let s = container_site(&range).await;
    let admin = s.admin_cookie().await;
    let page = s.get("/admin", Some(&admin)).await.body;
    assert!(
        page.contains("reachable, and it accepts this site"),
        "{page}"
    );

    let s = build_full(
        None,
        None,
        &[],
        ALL_RELEASED,
        Some(crate::live::LiveConfig {
            domain: LIVE_DOMAIN.into(),
            host_header: None,
            range: Some(crate::live::Range::new(
                &range.url,
                "wrong-token-0123456789abcdef",
            )),
        }),
        crate::captcha::Captcha::Off,
    )
    .await;
    let admin = s.admin_cookie().await;
    assert!(s
        .get("/admin", Some(&admin))
        .await
        .body
        .contains("rejects this site"));

    // A site with live puzzles and no range says so.
    let s = live_site().await;
    let admin = s.admin_cookie().await;
    assert!(s
        .get("/admin", Some(&admin))
        .await
        .body
        .contains("No range is set"));
}

#[test]
fn a_failed_start_logs_why_the_range_could_not_be_reached() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let log = capture_logs(|| {
        rt.block_on(async {
            let dead = {
                let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                format!("http://{}", l.local_addr().unwrap())
            };
            let s = build_full(
                None,
                None,
                &[],
                ALL_RELEASED,
                Some(crate::live::LiveConfig {
                    domain: LIVE_DOMAIN.into(),
                    host_header: None,
                    range: Some(crate::live::Range::new(&dead, RANGE_TOKEN)),
                }),
                crate::captcha::Captcha::Off,
            )
            .await;
            s.register_verified("log-user", "log@example.org").await;
            let cookie = s.login_cookie("log-user").await;
            s.send(
                Request::post("/api/p/webshell-lab/instance")
                    .header(header::COOKIE, &cookie)
                    .header(header::ORIGIN, ORIGIN)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        })
    });
    assert!(log.contains("the range refused"), "{log}");
    assert!(
        log.to_lowercase().contains("refused"),
        "the cause is in the log: {log}"
    );
    assert!(
        log.contains("Connection") || log.contains("connect"),
        "{log}"
    );
}

#[tokio::test]
async fn a_lab_that_was_never_shipped_says_so() {
    let range = stub_range().await;
    // The range is reached and the token is good, but it has no such lab.
    *range.refuse.lock().unwrap() = Some(StatusCode::BAD_REQUEST);
    let s = container_site(&range).await;
    let admin = s.admin_cookie().await;
    let r = s
        .send(admin_post("/admin/live/webshell-lab/start", Some(&admin)))
        .await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        r.body.contains("not been set up on the range"),
        "{}",
        r.body
    );
}

#[tokio::test]
async fn the_admin_page_says_whether_the_captcha_is_on() {
    let s = site().await;
    let admin = s.admin_cookie().await;
    assert!(
        s.get("/admin", Some(&admin)).await.body.contains("Captcha"),
        "shown even when off"
    );
    assert!(s
        .get("/admin", Some(&admin))
        .await
        .body
        .contains("Off. Set NECROPSY_TURNSTILE_SITE_KEY"));
    let s = captcha_site().await;
    let admin = s.admin_cookie().await;
    let page = s.get("/admin", Some(&admin)).await.body;
    assert!(page.contains("Cloudflare Turnstile, site key"), "{page}");
    assert!(
        page.contains("test-site-"),
        "the start of the key is shown, never the secret"
    );
}
