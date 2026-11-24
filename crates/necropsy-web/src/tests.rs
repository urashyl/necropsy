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
    let packs = necropsy_pack::load_all(std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../packs"
    )))
    .unwrap();
    let mail = Arc::new(Mutex::new(Vec::new()));
    let mailer = mailer.unwrap_or_else(|| Mailer::Memory(mail.clone()));
    let state = Arc::new(AppState::new(
        packs,
        Config {
            base_url: ORIGIN.into(),
            client_ip_header: client_ip_header.map(|h| h.parse().unwrap()),
        },
        Db::open(":memory:").unwrap(),
        mailer,
    ));
    Site {
        app: router(state.clone()),
        state,
        mail,
    }
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
        "/format",
        "/login",
        "/register",
        "/p/phone-home",
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
    let r = site().await.get("/p/phone-home", None).await;
    assert!(!r.body.contains("sync.pagepilot-cdn.invalid</code>"));
    assert!(!r.body.contains("The answer is"));
}

#[tokio::test]
async fn wrong_answer_gets_no_explainer_and_right_answer_does() {
    let s = site().await;
    let r = s
        .post_json(
            "/api/p/phone-home/answer",
            r#"{"answer":"stats.pagepilot-assets.invalid"}"#,
            None,
        )
        .await;
    assert!(r.body.contains("\"correct\":false") && !r.body.contains("explainer_html"));
    let r = s
        .post_json(
            "/api/p/phone-home/answer",
            r#"{"answer":"  Sync.PagePilot-CDN.invalid "}"#,
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
        s.get("/api/p/phone-home/explainer", Some(&cookie))
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
