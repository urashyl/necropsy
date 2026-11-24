// SPDX-License-Identifier: AGPL-3.0-or-later
//! Authentication building blocks: credentials, tokens, cookies, rate
//! limiting, request extractors and the first-run admin account.
//!
//! See `docs/accounts.md` for the overall design and threat model.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use argon2::{Argon2, PasswordHasher, PasswordVerifier};
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use sha2::{Digest, Sha256};

use crate::db::{Db, User};
use crate::state::AppState;

pub const COOKIE_NAME: &str = "necropsy_session";
pub const SESSION_TTL_SECS: i64 = 30 * 24 * 3600;
pub const VERIFY_TTL_SECS: i64 = 48 * 3600;
pub const RESET_TTL_SECS: i64 = 3600;
pub const CHANGE_EMAIL_TTL_SECS: i64 = 24 * 3600;

pub const MIN_PASSWORD_LEN: usize = 10;
pub const MAX_PASSWORD_LEN: usize = 128;

// ---------------------------------------------------------------- randomness

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    // The OS random source failing means the machine is unusable for
    // generating credentials; refusing to continue is the safe response.
    getrandom::fill(&mut buf).expect("operating system random source unavailable");
    buf
}

/// A fresh 256-bit token as 64 hex characters, for cookies and email links.
pub fn new_token() -> String {
    hex(&random_bytes::<32>())
}

/// What is stored for a token. Only the hash goes in the database, so a
/// database leak does not yield live sessions or reset links.
pub fn token_hash(token: &str) -> String {
    hex(&Sha256::digest(token.as_bytes()))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// `len` random characters from `alphabet`, without modulo bias.
fn random_string(alphabet: &[u8], len: usize) -> String {
    let limit = 256 - (256 % alphabet.len());
    let mut out = String::with_capacity(len);
    while out.len() < len {
        for b in random_bytes::<64>() {
            if (b as usize) < limit && out.len() < len {
                out.push(alphabet[b as usize % alphabet.len()] as char);
            }
        }
    }
    out
}

// ---------------------------------------------------------------- passwords

pub fn hash_password_blocking(password: &str) -> Result<String, ()> {
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|h| h.to_string())
        .map_err(|_| ())
}

pub fn verify_password_blocking(password: &str, hash: &str) -> bool {
    Argon2::default()
        .verify_password(password.as_bytes(), hash)
        .is_ok()
}

/// Argon2id is deliberately slow, so it runs off the async runtime.
pub async fn hash_password(password: String) -> Result<String, ()> {
    tokio::task::spawn_blocking(move || hash_password_blocking(&password))
        .await
        .map_err(|_| ())?
}

pub async fn verify_password(password: String, hash: String) -> bool {
    tokio::task::spawn_blocking(move || verify_password_blocking(&password, &hash))
        .await
        .unwrap_or(false)
}

// ---------------------------------------------------------------- validation

const RESERVED_PREFIXES: &[&str] = &[
    "admin",
    "root",
    "necropsy",
    "system",
    "support",
    "staff",
    "moderator",
];

/// Length and character rules, which apply to everyone.
pub fn validate_username_format(u: &str) -> Result<(), &'static str> {
    if !(3..=24).contains(&u.len()) {
        return Err("Username must be 3 to 24 characters.");
    }
    if !u
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err("Username can only contain letters, numbers, - and _.");
    }
    if !u.as_bytes()[0].is_ascii_alphanumeric() {
        return Err("Username must start with a letter or number.");
    }
    Ok(())
}

/// Rules for registering: the format, plus reserved names. The admin can
/// choose a reserved name for their own account (see `account_username_post`)
/// because registration is the only way anyone else could take it.
pub fn validate_username(u: &str) -> Result<(), &'static str> {
    validate_username_format(u)?;
    let lower = u.to_ascii_lowercase();
    if RESERVED_PREFIXES.iter().any(|p| lower.starts_with(p)) {
        return Err("That username is reserved.");
    }
    Ok(())
}

pub fn validate_password(p: &str) -> Result<(), &'static str> {
    let n = p.chars().count();
    if n < MIN_PASSWORD_LEN {
        return Err("Password must be at least 10 characters.");
    }
    if p.len() > MAX_PASSWORD_LEN {
        return Err("Password must be at most 128 bytes.");
    }
    Ok(())
}

/// Trimmed, lowercased address, or `None` if it is not a plausible address.
pub fn normalize_email(raw: &str) -> Option<String> {
    let e = raw.trim().to_lowercase();
    if e.len() > 254 || e.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return None;
    }
    let addr = lettre::Address::from_str(&e).ok()?;
    addr.domain().contains('.').then(|| addr.to_string())
}

// ---------------------------------------------------------------- rate limits

/// Sliding-window limiter kept in memory. State is lost on restart, which is
/// acceptable for brute-force and abuse throttling.
#[derive(Default)]
pub struct RateLimiter {
    hits: Mutex<HashMap<String, Vec<Instant>>>,
}

impl RateLimiter {
    /// Records an attempt for `key` and returns whether it is within `max`
    /// attempts per `window`. Rejected attempts are not recorded, so waiting
    /// out the window always works.
    pub fn allow(&self, key: &str, max: usize, window: Duration) -> bool {
        let mut map = self.hits.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let v = map.entry(key.to_owned()).or_default();
        v.retain(|t| now.duration_since(*t) < window);
        if v.len() >= max {
            return false;
        }
        v.push(now);
        true
    }

    /// Drops keys with no recent hits so the map cannot grow without bound.
    pub fn prune(&self, window: Duration) {
        let mut map = self.hits.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        map.retain(|_, v| v.iter().any(|t| now.duration_since(*t) < window));
    }
}

pub const AUTH_WINDOW: Duration = Duration::from_secs(15 * 60);

// ---------------------------------------------------------------- cookies

pub fn session_cookie(token: &str, secure: bool) -> String {
    format!(
        "{COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={SESSION_TTL_SECS}{}",
        if secure { "; Secure" } else { "" }
    )
}

pub fn clear_cookie(secure: bool) -> String {
    format!(
        "{COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{}",
        if secure { "; Secure" } else { "" }
    )
}

pub fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|pair| {
            let (k, v) = pair.trim().split_once('=')?;
            (k == name).then(|| v.to_owned())
        })
}

// ---------------------------------------------------------------- extractors

/// The signed-in user, if any. Never fails.
pub struct MaybeUser(pub Option<User>);

impl FromRequestParts<Arc<AppState>> for MaybeUser {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let Some(token) = cookie_value(&parts.headers, COOKIE_NAME) else {
            return Ok(MaybeUser(None));
        };
        if token.len() != 64 {
            return Ok(MaybeUser(None));
        }
        match state.db.session_user(token_hash(&token)).await {
            Ok(u) => Ok(MaybeUser(u)),
            Err(e) => {
                tracing::error!("session lookup failed: {e}");
                Ok(MaybeUser(None))
            }
        }
    }
}

/// A signed-in admin. Anonymous visitors are sent to the login page;
/// signed-in non-admins get a plain 404 so the page's existence is not
/// confirmed to them.
pub struct AdminUser(pub User);

impl FromRequestParts<Arc<AppState>> for AdminUser {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let MaybeUser(user) = MaybeUser::from_request_parts(parts, state)
            .await
            .unwrap_or(MaybeUser(None));
        match user {
            Some(u) if u.is_admin => Ok(AdminUser(u)),
            Some(_) => Err(StatusCode::NOT_FOUND.into_response()),
            None => Err(Redirect::to("/login").into_response()),
        }
    }
}

/// Best-effort client address for rate limiting, set by `client_ip_layer`.
#[derive(Clone)]
pub struct ClientIp(pub String);

impl<S: Send + Sync> FromRequestParts<S> for ClientIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(parts
            .extensions
            .get::<ClientIp>()
            .cloned()
            .unwrap_or_else(|| ClientIp("unknown".into())))
    }
}

// ---------------------------------------------------------------- first run

/// Creates the admin account if there is none, returning its generated
/// credentials. They are random and exist nowhere but the returned value, so
/// the caller must print them: there is no default account and no way to
/// recover them later. The admin has no email address, so password reset by
/// email does not apply to it.
pub async fn bootstrap_admin(db: &Db) -> Result<Option<(String, String)>, String> {
    if db.has_admin().await.map_err(|e| e.to_string())? {
        return Ok(None);
    }
    let username = format!(
        "admin-{}",
        random_string(b"abcdefghjkmnpqrstuvwxyz23456789", 6)
    );
    // No look-alike characters (0/O, 1/l/I) since this is read from a log.
    let password = random_string(
        b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnpqrstuvwxyz23456789",
        24,
    );
    let hash = hash_password(password.clone())
        .await
        .map_err(|_| "could not hash the admin password".to_owned())?;
    db.create_user(username.clone(), None, hash, true, true)
        .await
        .map_err(|e| format!("could not create the admin account: {e:?}"))?;
    Ok(Some((username, password)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn username_rules() {
        assert!(validate_username("mitch_01").is_ok());
        assert!(validate_username("ab").is_err());
        assert!(validate_username("has space").is_err());
        assert!(validate_username("-leading").is_err());
        assert!(validate_username("Admin-x").is_err());
        assert!(validate_username("rootkit").is_err());
        assert!(validate_username_format("admin").is_ok());
        assert!(validate_username_format("a b").is_err());
        assert!(validate_username(&"a".repeat(25)).is_err());
    }

    #[test]
    fn password_rules() {
        assert!(validate_password("short").is_err());
        assert!(validate_password("long enough pw").is_ok());
        assert!(validate_password(&"x".repeat(129)).is_err());
    }

    #[test]
    fn email_normalization() {
        assert_eq!(
            normalize_email("  Me@Example.COM "),
            Some("me@example.com".into())
        );
        assert_eq!(normalize_email("nope"), None);
        assert_eq!(normalize_email("a@b"), None);
        assert_eq!(normalize_email("a b@example.com"), None);
        assert_eq!(normalize_email("a@example.com\r\nBcc: x@y.z"), None);
    }

    #[test]
    fn tokens_are_unique_and_hashed() {
        let (a, b) = (new_token(), new_token());
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
        assert_ne!(token_hash(&a), a);
        assert_eq!(token_hash(&a), token_hash(&a));
    }

    #[test]
    fn password_roundtrip() {
        let h = hash_password_blocking("correct horse battery").unwrap();
        assert!(verify_password_blocking("correct horse battery", &h));
        assert!(!verify_password_blocking("wrong", &h));
    }

    #[test]
    fn rate_limiter_blocks_then_recovers() {
        let rl = RateLimiter::default();
        let w = Duration::from_millis(50);
        assert!(rl.allow("k", 2, w));
        assert!(rl.allow("k", 2, w));
        assert!(!rl.allow("k", 2, w));
        assert!(rl.allow("other", 2, w));
        std::thread::sleep(Duration::from_millis(60));
        assert!(rl.allow("k", 2, w));
    }

    #[test]
    fn cookie_parsing() {
        let mut h = HeaderMap::new();
        h.insert(
            header::COOKIE,
            "a=1; necropsy_session=abc; b=2".parse().unwrap(),
        );
        assert_eq!(cookie_value(&h, COOKIE_NAME).as_deref(), Some("abc"));
        assert_eq!(cookie_value(&h, "missing"), None);
    }
}
