// SPDX-License-Identifier: AGPL-3.0-or-later
//! Two-step verification: an authenticator app (TOTP, RFC 6238) and codes sent by
//! email, with one-time recovery codes. See `docs/accounts.md`.
//!
//! A password login for an account with a second step does not start a session. It
//! leaves a short-lived challenge (a row in `mfa_tickets`, named by a cookie), and the
//! session starts only when a valid code is given for that challenge. A challenge
//! allows a few wrong codes, then is dropped.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{AppendHeaders, IntoResponse, Redirect, Response};
use axum::Form;
use hmac::{Hmac, Mac};
use maud::{html, Markup, PreEscaped};
use serde::Deserialize;
use sha1::Sha1;

use crate::accounts::{account_page, confirm_password, form_page, notice_page, too_many};
use crate::auth::*;
use crate::db::{now, User};
use crate::pages::{layout, Nav};
use crate::state::AppState;

/// Seconds a login challenge lives.
const TICKET_TTL: i64 = 600;
/// Wrong codes a challenge allows.
const MAX_ATTEMPTS: i64 = 5;
/// An emailed code's lifetime, how soon another may be sent, and how many a challenge gets.
const EMAIL_CODE_TTL: i64 = 600;
const EMAIL_COOLDOWN: i64 = 30;
const EMAIL_MAX_SENDS: i64 = 3;
/// How long an authenticator setup stays open.
const SETUP_TTL: i64 = 900;
const RECOVERY_CODES: usize = 10;

// ------------------------------------------------------------------------ TOTP

const STEP_SECS: i64 = 30;

const B32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// RFC 4648 base32, upper case, no padding.
pub fn base32(bytes: &[u8]) -> String {
    let mut out = String::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    for &b in bytes {
        acc = (acc << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(B32[((acc >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(B32[((acc << (5 - bits)) & 31) as usize] as char);
    }
    out
}

pub fn unbase32(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes().filter(|c| *c != b'=' && *c != b' ') {
        let v = B32.iter().position(|&x| x == c.to_ascii_uppercase())? as u32;
        acc = (acc << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

/// A new 160-bit secret, base32 encoded: what an authenticator app is given.
pub fn new_secret() -> String {
    base32(&random_secret_bytes::<20>())
}

/// RFC 4226 HOTP, six digits.
fn hotp(secret: &[u8], counter: u64) -> String {
    let mut mac = Hmac::<Sha1>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(&counter.to_be_bytes());
    let h = mac.finalize().into_bytes();
    let o = (h[19] & 0x0f) as usize;
    let n = u32::from_be_bytes([h[o] & 0x7f, h[o + 1], h[o + 2], h[o + 3]]) % 1_000_000;
    format!("{n:06}")
}

/// The code an authenticator app shows at unix time `t`. (Tests only: the server
/// checks codes, it never makes them.)
#[cfg(test)]
pub fn totp_code(secret: &[u8], t: i64) -> String {
    hotp(secret, (t.div_euclid(STEP_SECS)) as u64)
}

/// The time step whose code is `code`, within one step either side of `t`, if any.
pub fn totp_match(secret: &[u8], code: &str, t: i64) -> Option<i64> {
    let step = t.div_euclid(STEP_SECS);
    (step - 1..=step + 1).find(|s| ct_eq(hotp(secret, *s as u64).as_bytes(), code.as_bytes()))
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

// ------------------------------------------------------------- recovery codes

const CODE_ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";

/// Ten codes like `k3f9-m2xq`, and their hashes for storage.
fn new_recovery_codes() -> (Vec<String>, Vec<String>) {
    let codes: Vec<String> = (0..RECOVERY_CODES)
        .map(|_| {
            let c = random_string(CODE_ALPHABET, 8);
            format!("{}-{}", &c[..4], &c[4..])
        })
        .collect();
    let hashes = codes.iter().map(|c| recovery_hash(c)).collect();
    (codes, hashes)
}

fn recovery_hash(code: &str) -> String {
    token_hash(&normalize_recovery(code))
}

fn normalize_recovery(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn looks_like_recovery(code: &str) -> bool {
    normalize_recovery(code).len() == 8
}

fn six_digits(code: &str) -> Option<String> {
    let d: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    (d.len() == 6 && d.bytes().all(|b| b.is_ascii_digit())).then_some(d)
}

fn email_code_hash(user_id: i64, code: &str) -> String {
    token_hash(&format!("mfa-email:{user_id}:{code}"))
}

// ---------------------------------------------------------------- the challenge

fn mfa_cookie_name(secure: bool) -> &'static str {
    if secure {
        "__Host-necropsy_mfa"
    } else {
        "necropsy_mfa"
    }
}

fn mfa_cookie(token: &str, secure: bool) -> String {
    format!(
        "{}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={TICKET_TTL}{}",
        mfa_cookie_name(secure),
        if secure { "; Secure" } else { "" }
    )
}

fn clear_mfa_cookie(secure: bool) -> String {
    format!(
        "{}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{}",
        mfa_cookie_name(secure),
        if secure { "; Secure" } else { "" }
    )
}

/// Called after a correct password for an account with a second step: leaves a
/// challenge and sends the browser to answer it.
pub async fn begin_login(state: &AppState, user: &User) -> Response {
    let token = new_token();
    if let Err(e) = state
        .db
        .mfa_ticket_put(token_hash(&token), user.id, now() + TICKET_TTL)
        .await
    {
        tracing::error!("storing the login challenge failed: {e}");
        return notice_page(
            state,
            None,
            StatusCode::INTERNAL_SERVER_ERROR,
            "Something went wrong",
            "Try again in a moment.",
            None,
        );
    }
    (
        [(header::SET_COOKIE, mfa_cookie(&token, state.secure_cookies))],
        Redirect::to("/login/verify"),
    )
        .into_response()
}

async fn ticket_of(
    state: &AppState,
    headers: &HeaderMap,
) -> Option<(String, crate::db::MfaTicket)> {
    let token = cookie_value(headers, mfa_cookie_name(state.secure_cookies))?;
    let hash = token_hash(&token);
    let ticket = state.db.mfa_ticket_get(hash.clone()).await.ok().flatten()?;
    Some((hash, ticket))
}

#[derive(Deserialize, Default)]
pub struct VerifyQuery {
    sent: Option<String>,
}

fn verify_form(
    user: &User,
    notice: Option<&str>,
    error: Option<&str>,
    state: &AppState,
) -> Response {
    let status = if error.is_some() {
        StatusCode::UNAUTHORIZED
    } else {
        StatusCode::OK
    };
    let hint = match (user.totp, user.mfa_email) {
        (true, _) => "From your authenticator app, or a recovery code.",
        (false, _) => "The code we emailed you, or a recovery code.",
    };
    form_page(
        state,
        None,
        status,
        "Two-step verification",
        error,
        html! {
            @if let Some(n) = notice { p.form-ok role="status" { (n) } }
            form method="post" action="/login/verify" {
                div.field {
                    label for="code" { "Code" }
                    input #code name="code" type="text" required autocomplete="one-time-code"
                        inputmode="text" autocapitalize="off" spellcheck="false" maxlength="32" autofocus;
                    p.field-hint { (hint) }
                }
                button.button type="submit" { "Verify" }
            }
            @if user.mfa_email {
                form.mfa-alt method="post" action="/login/verify/email" {
                    button.button.button-quiet type="submit" { "Email me a code" }
                }
            }
            p.auth-links { a href="/login" { "Back to log in" } }
        },
    )
}

async fn challenge_user(state: &AppState, user_id: i64) -> Option<User> {
    state
        .db
        .user_by_id(user_id)
        .await
        .ok()
        .flatten()
        .filter(|u| !u.disabled && u.mfa())
}

pub async fn login_verify_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<VerifyQuery>,
) -> Response {
    let Some((_, ticket)) = ticket_of(&state, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    let Some(user) = challenge_user(&state, ticket.user_id).await else {
        return Redirect::to("/login").into_response();
    };
    let notice = q
        .sent
        .is_some()
        .then_some("Code sent. It works for 10 minutes.");
    verify_form(&user, notice, None, &state)
}

#[derive(Deserialize)]
pub struct CodeForm {
    code: String,
}

/// What a code was accepted as.
#[derive(PartialEq, Eq, Debug)]
enum Factor {
    Authenticator,
    Email,
    Recovery,
}

/// Checks a code against the user's authenticator, the emailed code on `ticket`,
/// and their recovery codes. A code is spent when it is accepted.
async fn check_code(
    state: &AppState,
    user: &User,
    ticket: Option<(&str, &crate::db::MfaTicket)>,
    code: &str,
) -> Option<Factor> {
    if let Some(digits) = six_digits(code) {
        if user.totp {
            if let Ok(Some((secret, last))) = state.db.totp_state(user.id).await {
                let secret = unbase32(&secret)?;
                if let Some(step) = totp_match(&secret, &digits, now()) {
                    // A step is good once: a code seen on the wire cannot be replayed.
                    if step > last && state.db.totp_use_step(user.id, step).await.unwrap_or(false) {
                        return Some(Factor::Authenticator);
                    }
                    return None;
                }
            }
        }
        if let Some((hash, t)) = ticket {
            if let Some(want) = &t.email_hash {
                if t.email_expires > now()
                    && ct_eq(
                        want.as_bytes(),
                        email_code_hash(user.id, &digits).as_bytes(),
                    )
                {
                    let _ = state
                        .db
                        .mfa_ticket_email(hash.to_owned(), String::new(), 0)
                        .await;
                    return Some(Factor::Email);
                }
            }
        }
        return None;
    }
    if looks_like_recovery(code)
        && state
            .db
            .recovery_use(user.id, recovery_hash(code))
            .await
            .unwrap_or(false)
    {
        return Some(Factor::Recovery);
    }
    None
}

pub async fn login_verify_post(
    State(state): State<Arc<AppState>>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Form(f): Form<CodeForm>,
) -> Response {
    let Some((hash, ticket)) = ticket_of(&state, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    let Some(user) = challenge_user(&state, ticket.user_id).await else {
        return Redirect::to("/login").into_response();
    };
    if !state
        .limiter
        .allow(&format!("mfa-ip:{ip}"), 30, AUTH_WINDOW)
        || !state
            .limiter
            .allow(&format!("mfa-user:{}", user.id), 15, AUTH_WINDOW)
    {
        return too_many(&state, None);
    }
    match check_code(&state, &user, Some((hash.as_str(), &ticket)), &f.code).await {
        Some(factor) => {
            let _ = state.db.mfa_ticket_delete(hash).await;
            if factor == Factor::Recovery {
                let left = state.db.recovery_count(user.id).await.unwrap_or(0);
                crate::accounts::notify(
                    &state,
                    user.id,
                    "A recovery code was used",
                    format!(
                        "A recovery code was used to log in to your necropsy account. \
                         {left} are left. If this was not you, change your password.\n"
                    ),
                )
                .await;
            }
            match crate::accounts::start_session(&state, user.id).await {
                Ok(cookie) => (
                    AppendHeaders([
                        (header::SET_COOKIE, cookie),
                        (header::SET_COOKIE, clear_mfa_cookie(state.secure_cookies)),
                    ]),
                    Redirect::to("/"),
                )
                    .into_response(),
                Err(()) => notice_page(
                    &state,
                    None,
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Something went wrong",
                    "Try again in a moment.",
                    None,
                ),
            }
        }
        None => {
            let attempts = state
                .db
                .mfa_ticket_fail(hash.clone())
                .await
                .unwrap_or(i64::MAX);
            if attempts >= MAX_ATTEMPTS {
                let _ = state.db.mfa_ticket_delete(hash).await;
                return (
                    [(header::SET_COOKIE, clear_mfa_cookie(state.secure_cookies))],
                    form_page(
                        &state,
                        None,
                        StatusCode::TOO_MANY_REQUESTS,
                        "Log in",
                        Some("Too many wrong codes. Log in again."),
                        html! { p.auth-links { a href="/login" { "Log in" } } },
                    ),
                )
                    .into_response();
            }
            verify_form(&user, None, Some("That code did not work."), &state)
        }
    }
}

pub async fn login_verify_email_post(
    State(state): State<Arc<AppState>>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
) -> Response {
    let Some((hash, ticket)) = ticket_of(&state, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    let Some(user) = challenge_user(&state, ticket.user_id).await else {
        return Redirect::to("/login").into_response();
    };
    if !user.mfa_email || !state.mailer.enabled() {
        return verify_form(&user, None, Some("Email codes are not available."), &state);
    }
    if !state
        .limiter
        .allow(&format!("mfa-mail-ip:{ip}"), 10, AUTH_WINDOW)
    {
        return too_many(&state, None);
    }
    if ticket.email_sends >= EMAIL_MAX_SENDS || now() - ticket.email_sent_at < EMAIL_COOLDOWN {
        return verify_form(
            &user,
            None,
            Some("A code was sent a moment ago. Wait, then try again."),
            &state,
        );
    }
    let code = format!("{:06}", random_index(1_000_000));
    if state
        .db
        .mfa_ticket_email(
            hash,
            email_code_hash(user.id, &code),
            now() + EMAIL_CODE_TTL,
        )
        .await
        .is_err()
    {
        return verify_form(
            &user,
            None,
            Some("Could not send a code. Try again."),
            &state,
        );
    }
    crate::accounts::notify(
        &state,
        user.id,
        "Your necropsy login code",
        format!(
            "Your login code is {code}. It works for 10 minutes.\n\
             If you did not try to log in, change your password.\n"
        ),
    )
    .await;
    Redirect::to("/login/verify?sent=1").into_response()
}

// -------------------------------------------------------------- account settings

/// The "Two-step verification" part of the account page.
pub async fn settings_section(state: &AppState, user: &User) -> Markup {
    let has_email =
        state.mailer.enabled() && state.db.email_of(user.id).await.ok().flatten().is_some();
    let codes_left = state.db.recovery_count(user.id).await.unwrap_or(0);
    html! {
        h2 { "Two-step verification" }

        h3 { "Authenticator app" }
        @if user.totp {
            p { "On." }
            form method="post" action="/account/mfa/totp/remove" {
                (password_field("totp-remove-password"))
                (code_field("Authenticator or recovery code", "totp-remove-code"))
                button.button.button-quiet type="submit" { "Remove" }
            }
        } @else {
            p { "Off." }
            form method="post" action="/account/mfa/totp/start" {
                (password_field("totp-start-password"))
                button.button.button-quiet type="submit" { "Set up" }
            }
        }

        @if has_email || user.mfa_email {
            h3 { "Email codes" }
            @if user.mfa_email {
                p { "On." }
                form method="post" action="/account/mfa/email/disable" {
                    (password_field("email-off-password"))
                    @if user.totp {
                        (code_field("Authenticator or recovery code", "email-off-code"))
                    }
                    button.button.button-quiet type="submit" { "Turn off" }
                }
            } @else {
                p { "Off." }
                form method="post" action="/account/mfa/email/enable" {
                    (password_field("email-on-password"))
                    button.button.button-quiet type="submit" { "Turn on" }
                }
            }
        }

        @if user.mfa() {
            h3 { "Recovery codes" }
            p { (codes_left) " left. Each works once." }
            form method="post" action="/account/mfa/recovery" {
                (password_field("recovery-password"))
                @if user.totp {
                    (code_field("Authenticator or recovery code", "recovery-code"))
                }
                button.button.button-quiet type="submit" { "Make new codes" }
            }
        }
    }
}

/// The password box of a settings form: the handlers read it as `password`.
fn password_field(id: &str) -> Markup {
    html! {
        div.field {
            label for=(id) { "Password" }
            input #(id) name="password" type="password" required autocomplete="current-password"
                autocapitalize="off" spellcheck="false" maxlength="256";
        }
    }
}

fn code_field(label: &str, id: &str) -> Markup {
    html! {
        div.field {
            label for=(id) { (label) }
            input #(id) name="code" type="text" required autocomplete="one-time-code"
                autocapitalize="off" spellcheck="false" maxlength="32";
        }
    }
}

/// Re-checks a code for a settings change. With an authenticator, a current code or
/// an unused recovery code is required; without one, the password was enough.
async fn require_code(state: &AppState, user: &User, code: &str) -> Result<(), Box<Response>> {
    if !user.totp {
        return Ok(());
    }
    if !state
        .limiter
        .allow(&format!("mfa-account:{}", user.id), 10, AUTH_WINDOW)
    {
        return Err(Box::new(
            account_page(
                state,
                user,
                Some((false, "Too many attempts. Wait a few minutes.")),
                StatusCode::TOO_MANY_REQUESTS,
            )
            .await,
        ));
    }
    match check_code(state, user, None, code).await {
        Some(_) => Ok(()),
        None => Err(Box::new(
            account_page(
                state,
                user,
                Some((false, "That code did not work.")),
                StatusCode::FORBIDDEN,
            )
            .await,
        )),
    }
}

async fn ok_flash(state: &AppState, user: &User, text: &str) -> Response {
    // Re-read the user so the page shows what is stored now.
    let fresh = state.db.user_by_id(user.id).await.ok().flatten();
    account_page(
        state,
        fresh.as_ref().unwrap_or(user),
        Some((true, text)),
        StatusCode::OK,
    )
    .await
}

/// Shows recovery codes, once, after they are made.
async fn show_codes(state: &AppState, user: &User, codes: &[String]) -> Response {
    let body = html! {
        section.wrap.page-head { h1 { "Recovery codes" } }
        section.wrap.section-tight {
            div.auth {
                p { "Save these now. They are not shown again. Each works once, if you lose your phone." }
                pre.recovery-codes { @for c in codes { (c) "\n" } }
                p.auth-links { a href="/account" { "Back to account" } }
            }
        }
    };
    let fresh = state.db.user_by_id(user.id).await.ok().flatten();
    (
        StatusCode::OK,
        layout(
            state,
            Some(fresh.as_ref().unwrap_or(user)),
            "Recovery codes",
            "Recovery codes.",
            Nav::Account,
            "/account",
            body,
        ),
    )
        .into_response()
}

async fn make_codes(state: &AppState, user: &User) -> Option<Vec<String>> {
    let (codes, hashes) = new_recovery_codes();
    state.db.recovery_replace(user.id, hashes).await.ok()?;
    Some(codes)
}

/// After a second step is removed: with none left, the recovery codes go too.
async fn tidy_after_removal(state: &AppState, user_id: i64) {
    if let Ok(Some(u)) = state.db.user_by_id(user_id).await {
        if !u.mfa() {
            let _ = state.db.mfa_clear(user_id).await;
        }
    }
}

#[derive(Deserialize)]
pub struct PasswordForm {
    password: String,
}

#[derive(Deserialize)]
pub struct PasswordCodeForm {
    password: String,
    #[serde(default)]
    code: String,
}

pub async fn totp_start_post(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    Form(f): Form<PasswordForm>,
) -> Response {
    let Some(user) = viewer else {
        return Redirect::to("/login").into_response();
    };
    if let Err(r) = confirm_password(&state, &user, f.password).await {
        return *r;
    }
    if user.totp {
        return ok_flash(&state, &user, "The authenticator app is already on.").await;
    }
    let secret = new_secret();
    if state
        .db
        .mfa_setup_put(user.id, secret.clone(), now() + SETUP_TTL)
        .await
        .is_err()
    {
        return account_page(
            &state,
            &user,
            Some((false, "Something went wrong. Try again.")),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .await;
    }
    setup_page(&state, &user, &secret, None)
}

fn qr_svg(data: &str) -> Option<String> {
    use qrcode::render::svg;
    let svg = qrcode::QrCode::new(data.as_bytes())
        .ok()?
        .render::<svg::Color>()
        .min_dimensions(200, 200)
        .quiet_zone(true)
        .dark_color(svg::Color("#000000"))
        .light_color(svg::Color("#ffffff"))
        .build();
    // The renderer starts with an XML header and a doctype, which do not belong inline.
    svg.find("<svg").map(|i| svg[i..].to_owned())
}

fn setup_page(state: &AppState, user: &User, secret: &str, error: Option<&str>) -> Response {
    let uri = format!(
        "otpauth://totp/necropsy:{}?secret={secret}&issuer=necropsy&algorithm=SHA1&digits=6&period=30",
        user.username
    );
    let svg = qr_svg(&uri);
    let body = html! {
        section.wrap.page-head { h1 { "Set up an authenticator app" } }
        section.wrap.section-tight {
            div.auth {
                @if let Some(e) = error { p.form-error role="alert" { (e) } }
                @if let Some(svg) = svg {
                    div.qr role="img" aria-label="QR code for the authenticator app" { (PreEscaped(svg)) }
                }
                p { "Scan the code, or enter this key by hand:" }
                p { code.totp-secret { (secret) } }
                form method="post" action="/account/mfa/totp/confirm" {
                    div.field {
                        label for="code" { "Code from the app" }
                        input #code name="code" type="text" required inputmode="numeric"
                            autocomplete="one-time-code" maxlength="8" autofocus;
                    }
                    button.button type="submit" { "Turn on" }
                }
                p.auth-links { a href="/account" { "Cancel" } }
            }
        }
    };
    (
        if error.is_some() {
            StatusCode::UNPROCESSABLE_ENTITY
        } else {
            StatusCode::OK
        },
        layout(
            state,
            Some(user),
            "Set up an authenticator app",
            "Set up an authenticator app.",
            Nav::Account,
            "/account",
            body,
        ),
    )
        .into_response()
}

pub async fn totp_confirm_post(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    Form(f): Form<CodeForm>,
) -> Response {
    let Some(user) = viewer else {
        return Redirect::to("/login").into_response();
    };
    if !state
        .limiter
        .allow(&format!("mfa-setup:{}", user.id), 10, AUTH_WINDOW)
    {
        return too_many(&state, Some(&user));
    }
    let Some(secret) = state.db.mfa_setup_get(user.id).await.ok().flatten() else {
        return account_page(
            &state,
            &user,
            Some((false, "Setup timed out. Start again.")),
            StatusCode::GONE,
        )
        .await;
    };
    let bytes = unbase32(&secret).unwrap_or_default();
    let Some(step) = six_digits(&f.code).and_then(|c| totp_match(&bytes, &c, now())) else {
        return setup_page(
            &state,
            &user,
            &secret,
            Some("That code did not match. Check the time on your phone and try again."),
        );
    };
    if state.db.totp_enable(user.id, secret, step).await.is_err() {
        return account_page(
            &state,
            &user,
            Some((false, "Something went wrong. Try again.")),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .await;
    }
    after_enable(&state, &user).await
}

/// Makes recovery codes the first time any second step is turned on.
async fn after_enable(state: &AppState, user: &User) -> Response {
    if state.db.recovery_count(user.id).await.unwrap_or(0) == 0 {
        if let Some(codes) = make_codes(state, user).await {
            return show_codes(state, user, &codes).await;
        }
    }
    ok_flash(state, user, "Turned on.").await
}

pub async fn totp_remove_post(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    Form(f): Form<PasswordCodeForm>,
) -> Response {
    let Some(user) = viewer else {
        return Redirect::to("/login").into_response();
    };
    if let Err(r) = confirm_password(&state, &user, f.password).await {
        return *r;
    }
    if let Err(r) = require_code(&state, &user, &f.code).await {
        return *r;
    }
    let _ = state.db.totp_disable(user.id).await;
    tidy_after_removal(&state, user.id).await;
    ok_flash(&state, &user, "Authenticator app removed.").await
}

pub async fn email_enable_post(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    Form(f): Form<PasswordForm>,
) -> Response {
    let Some(user) = viewer else {
        return Redirect::to("/login").into_response();
    };
    if let Err(r) = confirm_password(&state, &user, f.password).await {
        return *r;
    }
    let has_email =
        state.mailer.enabled() && state.db.email_of(user.id).await.ok().flatten().is_some();
    if !has_email {
        return account_page(
            &state,
            &user,
            Some((false, "This account has no email address to send codes to.")),
            StatusCode::BAD_REQUEST,
        )
        .await;
    }
    let _ = state.db.mfa_email_set(user.id, true).await;
    after_enable(&state, &user).await
}

pub async fn email_disable_post(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    Form(f): Form<PasswordCodeForm>,
) -> Response {
    let Some(user) = viewer else {
        return Redirect::to("/login").into_response();
    };
    if let Err(r) = confirm_password(&state, &user, f.password).await {
        return *r;
    }
    if let Err(r) = require_code(&state, &user, &f.code).await {
        return *r;
    }
    let _ = state.db.mfa_email_set(user.id, false).await;
    tidy_after_removal(&state, user.id).await;
    ok_flash(&state, &user, "Email codes turned off.").await
}

pub async fn recovery_post(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    Form(f): Form<PasswordCodeForm>,
) -> Response {
    let Some(user) = viewer else {
        return Redirect::to("/login").into_response();
    };
    if let Err(r) = confirm_password(&state, &user, f.password).await {
        return *r;
    }
    if !user.mfa() {
        return ok_flash(&state, &user, "Turn on a second step first.").await;
    }
    if let Err(r) = require_code(&state, &user, &f.code).await {
        return *r;
    }
    match make_codes(&state, &user).await {
        Some(codes) => show_codes(&state, &user, &codes).await,
        None => {
            account_page(
                &state,
                &user,
                Some((false, "Something went wrong. Try again.")),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
            .await
        }
    }
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn totp_matches_the_rfc_6238_vectors() {
        // RFC 6238 appendix B, SHA-1, secret "12345678901234567890", 8 digits there;
        // the last six are the same code.
        let secret = b"12345678901234567890";
        for (t, code) in [
            (59, "287082"),
            (1111111109, "081804"),
            (1111111111, "050471"),
            (1234567890, "005924"),
            (2000000000, "279037"),
        ] {
            assert_eq!(totp_code(secret, t), code, "t = {t}");
        }
    }

    #[test]
    fn base32_round_trips_and_matches_the_rfc() {
        // RFC 4648 test vectors.
        assert_eq!(base32(b"foobar"), "MZXW6YTBOI");
        assert_eq!(base32(b"fooba"), "MZXW6YTB");
        assert_eq!(unbase32("MZXW6YTBOI").unwrap(), b"foobar");
        assert_eq!(unbase32("mzxw 6ytb").unwrap(), b"fooba");
        assert!(unbase32("not base32!").is_none());
        let s = new_secret();
        assert_eq!(s.len(), 32);
        assert_eq!(base32(&unbase32(&s).unwrap()), s);
    }

    #[test]
    fn a_code_is_good_one_step_either_side() {
        let secret = b"12345678901234567890";
        let t = 1_000_000_000;
        let code = totp_code(secret, t);
        assert!(totp_match(secret, &code, t).is_some());
        assert!(totp_match(secret, &code, t + 30).is_some());
        assert!(totp_match(secret, &code, t - 30).is_some());
        assert!(totp_match(secret, &code, t + 90).is_none());
        assert!(totp_match(secret, "000000", t).is_none() || code == "000000");
    }

    #[test]
    fn recovery_codes_ignore_case_and_dashes() {
        let (codes, hashes) = new_recovery_codes();
        assert_eq!(codes.len(), RECOVERY_CODES);
        assert!(codes[0].len() == 9 && codes[0].as_bytes()[4] == b'-');
        assert_eq!(recovery_hash(&codes[0]), hashes[0]);
        assert_eq!(
            recovery_hash(&codes[0].to_uppercase().replace('-', " ")),
            hashes[0]
        );
        assert!(!looks_like_recovery("123456"));
        assert!(looks_like_recovery("abcd-efgh"));
    }
}
