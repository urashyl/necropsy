// SPDX-License-Identifier: AGPL-3.0-or-later
//! Account pages: register, verify, log in and out, password reset, and the
//! account settings page. See `docs/accounts.md`.
//!
//! Every form is a plain HTML POST that works without JavaScript. Browsers
//! are kept from cross-site posting by the Origin check in `main.rs` plus a
//! `SameSite=Lax` session cookie.
//!
//! Email addresses are write-only here: they are accepted on forms and sent
//! to the mailer, but no page renders one.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Form;
use maud::{html, Markup};
use serde::Deserialize;

use crate::auth::*;
use crate::db::{now, CreateError, User};
use crate::mail::Mail;
use crate::pages::{layout, Nav};
use crate::state::AppState;

fn ymd(ts: i64) -> String {
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = ts.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

pub fn format_date(ts: i64) -> String {
    ymd(ts)
}

// ---------------------------------------------------------------- helpers

pub(crate) fn field(
    label: &str,
    name: &str,
    kind: &str,
    autocomplete: &str,
    hint: Option<&str>,
) -> Markup {
    html! {
        div.field {
            label for=(name) { (label) }
            input #(name) name=(name) type=(kind) required autocomplete=(autocomplete)
                autocapitalize="off" spellcheck="false" maxlength="256";
            @if let Some(h) = hint { p.field-hint { (h) } }
        }
    }
}

pub(crate) fn form_page(
    state: &AppState,
    viewer: Option<&User>,
    status: StatusCode,
    title: &str,
    error: Option<&str>,
    form: Markup,
) -> Response {
    let body = html! {
        section.wrap.page-head { h1 { (title) } }
        section.wrap.section-tight {
            div.auth {
                @if let Some(e) = error { p.form-error role="alert" { (e) } }
                (form)
            }
        }
    };
    (
        status,
        layout(state, viewer, title, title, Nav::None, "/login", body),
    )
        .into_response()
}

pub(crate) fn notice_page(
    state: &AppState,
    viewer: Option<&User>,
    status: StatusCode,
    title: &str,
    text: &str,
    link: Option<(&str, &str)>,
) -> Response {
    let body = html! {
        section.wrap.page-head { h1 { (title) } }
        section.wrap.section-tight {
            div.auth {
                p { (text) }
                @if let Some((href, label)) = link {
                    p.auth-links { a href=(href) { (label) } }
                }
            }
        }
    };
    (
        status,
        layout(state, viewer, title, title, Nav::None, "/login", body),
    )
        .into_response()
}

pub(crate) fn too_many(state: &AppState, viewer: Option<&User>) -> Response {
    notice_page(
        state,
        viewer,
        StatusCode::TOO_MANY_REQUESTS,
        "Too many attempts",
        "Wait a few minutes and try again.",
        None,
    )
}

fn mail_closed(state: &AppState, viewer: Option<&User>, what: &str) -> Response {
    notice_page(
        state,
        viewer,
        StatusCode::SERVICE_UNAVAILABLE,
        what,
        "This server has no email configured, so this is unavailable.",
        None,
    )
}

fn server_error(state: &AppState, viewer: Option<&User>) -> Response {
    notice_page(
        state,
        viewer,
        StatusCode::INTERNAL_SERVER_ERROR,
        "Something went wrong",
        "Try again in a moment.",
        None,
    )
}

/// Stores a token and returns the plaintext to put in an email link.
async fn issue_token(
    state: &AppState,
    user_id: i64,
    purpose: &'static str,
    new_email: Option<String>,
    ttl: i64,
) -> Result<String, ()> {
    let token = new_token();
    state
        .db
        .put_token(user_id, token_hash(&token), purpose, new_email, now() + ttl)
        .await
        .map_err(|e| tracing::error!("storing {purpose} token failed: {e}"))?;
    Ok(token)
}

pub(crate) async fn start_session(state: &AppState, user_id: i64) -> Result<String, ()> {
    let token = new_token();
    state
        .db
        .create_session(user_id, token_hash(&token), now() + SESSION_TTL_SECS)
        .await
        .map_err(|e| tracing::error!("creating session failed: {e}"))?;
    Ok(session_cookie(&token, state.secure_cookies))
}

/// Best-effort notice to the address on file. Failures are logged, not shown.
pub(crate) async fn notify(state: &AppState, user_id: i64, subject: &str, body: String) {
    let Ok(Some(to)) = state.db.email_of(user_id).await else {
        return;
    };
    if let Err(e) = state
        .mailer
        .send(Mail {
            to,
            subject: subject.into(),
            body,
        })
        .await
    {
        tracing::warn!("could not send notice: {e}");
    }
}

fn mail_text(lines: &[&str]) -> String {
    let mut s = lines.join("\n");
    s.push('\n');
    s
}

// ---------------------------------------------------------------- register

pub async fn register_get(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
) -> Response {
    if !state.mailer.enabled() {
        return mail_closed(&state, viewer.as_ref(), "Register");
    }
    register_form(&state, viewer.as_ref(), StatusCode::OK, None)
}

fn register_form(
    state: &AppState,
    viewer: Option<&User>,
    status: StatusCode,
    error: Option<&str>,
) -> Response {
    form_page(
        state,
        viewer,
        status,
        "Register",
        error,
        html! {
            form method="post" action="/register" {
                (field("Username", "username", "text", "username", None))
                (field("Email", "email", "email", "email",
                    Some("Used to verify your account and reset your password. It is never shown to anyone.")))
                (field("Password", "password", "password", "new-password", Some("At least 10 characters.")))
                @if let Some(key) = state.captcha.site_key() {
                    div.cf-turnstile data-sitekey=(key) {}
                    script src="https://challenges.cloudflare.com/turnstile/v0/api.js" async defer {}
                }
                button.button type="submit" { "Create account" }
            }
            p.auth-links { "Already registered? " a href="/login" { "Log in" } }
        },
    )
}

#[derive(Deserialize)]
pub struct RegisterForm {
    username: String,
    email: String,
    password: String,
    /// Filled in by the Turnstile widget.
    #[serde(rename = "cf-turnstile-response", default)]
    captcha: String,
}

pub async fn register_post(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    ClientIp(ip): ClientIp,
    Form(f): Form<RegisterForm>,
) -> Response {
    let v = viewer.as_ref();
    if !state.mailer.enabled() {
        return mail_closed(&state, v, "Register");
    }
    if !state
        .limiter
        .allow(&format!("register:{ip}"), 5, AUTH_WINDOW)
    {
        return too_many(&state, v);
    }
    if !state.captcha.verify(&f.captcha, &ip).await {
        return register_form(
            &state,
            v,
            StatusCode::BAD_REQUEST,
            Some("The captcha check failed. Try again."),
        );
    }
    let username = f.username.trim().to_owned();
    if let Err(e) = validate_username(&username) {
        return register_form(&state, v, StatusCode::BAD_REQUEST, Some(e));
    }
    let Some(email) = normalize_email(&f.email) else {
        return register_form(
            &state,
            v,
            StatusCode::BAD_REQUEST,
            Some("Enter a valid email address."),
        );
    };
    if let Err(e) = validate_password(&f.password) {
        return register_form(&state, v, StatusCode::BAD_REQUEST, Some(e));
    }
    let Ok(hash) = hash_password(f.password).await else {
        return server_error(&state, v);
    };

    let sent = notice_page(
        &state,
        v,
        StatusCode::OK,
        "Check your email",
        "We sent a link to verify your account. It expires in 48 hours.",
        None,
    );

    let id = match state
        .db
        .create_user(username.clone(), Some(email.clone()), hash, false, false)
        .await
    {
        Ok(id) => id,
        Err(CreateError::UsernameTaken) => {
            return register_form(
                &state,
                v,
                StatusCode::CONFLICT,
                Some("That username is taken."),
            );
        }
        // Answer exactly as if it had worked, so this form cannot be used to
        // find out which addresses have accounts.
        Err(CreateError::EmailTaken) => return sent,
        Err(CreateError::Other(e)) => {
            tracing::error!("register failed: {e}");
            return server_error(&state, v);
        }
    };

    let Ok(token) = issue_token(&state, id, "verify", None, VERIFY_TTL_SECS).await else {
        let _ = state.db.delete_user(id).await;
        return server_error(&state, v);
    };
    let link = format!("{}/verify?token={token}", state.base_url);
    let mail = Mail {
        to: email,
        subject: "Verify your necropsy account".into(),
        body: mail_text(&[
            &format!("Hi {username},"),
            "",
            "Open this link to verify your account. It expires in 48 hours:",
            &link,
            "",
            "If you did not register, ignore this email.",
        ]),
    };
    if let Err(e) = state.mailer.send(mail).await {
        tracing::error!("could not send verification email: {e}");
        let _ = state.db.delete_user(id).await;
        return form_page(
            &state,
            v,
            StatusCode::BAD_GATEWAY,
            "Register",
            Some("Could not send the verification email. Try again later."),
            html! { p.auth-links { a href="/register" { "Back" } } },
        );
    }
    sent
}

#[derive(Deserialize)]
pub struct TokenQuery {
    token: Option<String>,
}

pub async fn verify_get(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    Query(q): Query<TokenQuery>,
) -> Response {
    let v = viewer.as_ref();
    let bad = || {
        notice_page(
            &state,
            v,
            StatusCode::BAD_REQUEST,
            "Link expired",
            "This verification link is invalid or has expired. Register again to get a new one.",
            Some(("/register", "Register")),
        )
    };
    let Some(token) = q.token.filter(|t| t.len() == 64) else {
        return bad();
    };
    match state.db.take_token(token_hash(&token), "verify").await {
        Ok(Some((user_id, _))) => {
            if state.db.set_verified(user_id).await.is_err() {
                return server_error(&state, v);
            }
            notice_page(
                &state,
                v,
                StatusCode::OK,
                "Email verified",
                "Your account is ready.",
                Some(("/login", "Log in")),
            )
        }
        Ok(None) => bad(),
        Err(e) => {
            tracing::error!("verify failed: {e}");
            server_error(&state, v)
        }
    }
}

// ---------------------------------------------------------------- login

pub async fn login_get(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
) -> Response {
    if viewer.is_some() {
        return Redirect::to("/account").into_response();
    }
    login_form(&state, None, StatusCode::OK, None)
}

fn login_form(
    state: &AppState,
    viewer: Option<&User>,
    status: StatusCode,
    error: Option<&str>,
) -> Response {
    form_page(
        state,
        viewer,
        status,
        "Log in",
        error,
        html! {
            form method="post" action="/login" {
                (field("Username", "username", "text", "username", None))
                (field("Password", "password", "password", "current-password", None))
                button.button type="submit" { "Log in" }
            }
            p.auth-links {
                @if state.mailer.enabled() {
                    a href="/forgot" { "Forgot password" } " · "
                    a href="/register" { "Register" }
                }
            }
        },
    )
}

#[derive(Deserialize)]
pub struct LoginForm {
    username: String,
    password: String,
}

pub async fn login_post(
    State(state): State<Arc<AppState>>,
    ClientIp(ip): ClientIp,
    Form(f): Form<LoginForm>,
) -> Response {
    let username = f.username.trim().to_owned();
    if !state
        .limiter
        .allow(&format!("login-ip:{ip}"), 30, AUTH_WINDOW)
        || !state.limiter.allow(
            &format!("login-user:{}", username.to_lowercase()),
            10,
            AUTH_WINDOW,
        )
    {
        return too_many(&state, None);
    }
    let wrong = || {
        login_form(
            &state,
            None,
            StatusCode::UNAUTHORIZED,
            Some("Wrong username or password."),
        )
    };

    let found = match state.db.user_for_login(username).await {
        Ok(f) => f,
        Err(e) => {
            tracing::error!("login lookup failed: {e}");
            return server_error(&state, None);
        }
    };
    let Some((user, hash)) = found else {
        verify_password(f.password, state.dummy_hash.clone()).await;
        return wrong();
    };
    if !verify_password(f.password, hash).await {
        return wrong();
    }
    if user.disabled {
        return login_form(
            &state,
            None,
            StatusCode::FORBIDDEN,
            Some("This account is disabled."),
        );
    }
    if !user.verified {
        return login_form(
            &state,
            None,
            StatusCode::FORBIDDEN,
            Some("Verify your email first. The link was sent when you registered."),
        );
    }
    if user.mfa() {
        return crate::mfa::begin_login(&state, &user).await;
    }
    match start_session(&state, user.id).await {
        Ok(cookie) => ([(header::SET_COOKIE, cookie)], Redirect::to("/")).into_response(),
        Err(()) => server_error(&state, None),
    }
}

pub async fn logout_post(
    State(state): State<Arc<AppState>>,
    req_headers: axum::http::HeaderMap,
) -> Response {
    if let Some(token) = cookie_value(&req_headers, cookie_name(state.secure_cookies)) {
        let _ = state.db.delete_session(token_hash(&token)).await;
    }
    (
        [(header::SET_COOKIE, clear_cookie(state.secure_cookies))],
        Redirect::to("/"),
    )
        .into_response()
}

// ---------------------------------------------------------------- reset

pub async fn forgot_get(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
) -> Response {
    if !state.mailer.enabled() {
        return mail_closed(&state, viewer.as_ref(), "Forgot password");
    }
    form_page(
        &state,
        viewer.as_ref(),
        StatusCode::OK,
        "Forgot password",
        None,
        html! {
            form method="post" action="/forgot" {
                (field("Email", "email", "email", "email", None))
                button.button type="submit" { "Send reset link" }
            }
        },
    )
}

#[derive(Deserialize)]
pub struct ForgotForm {
    email: String,
}

pub async fn forgot_post(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    ClientIp(ip): ClientIp,
    Form(f): Form<ForgotForm>,
) -> Response {
    let v = viewer.as_ref();
    if !state.mailer.enabled() {
        return mail_closed(&state, v, "Forgot password");
    }
    if !state.limiter.allow(&format!("forgot:{ip}"), 5, AUTH_WINDOW) {
        return too_many(&state, v);
    }
    // The reply is identical whether or not the address has an account.
    let reply = notice_page(
        &state,
        v,
        StatusCode::OK,
        "Check your email",
        "If an account uses that address, we sent a link to reset the password. It expires in one hour.",
        None,
    );
    let Some(email) = normalize_email(&f.email) else {
        return reply;
    };
    let Ok(Some(id)) = state.db.user_id_by_email(email.clone()).await else {
        return reply;
    };
    let usable = matches!(state.db.user_by_id(id).await, Ok(Some(u)) if u.verified && !u.disabled);
    if !usable {
        return reply;
    }
    if let Ok(token) = issue_token(&state, id, "reset", None, RESET_TTL_SECS).await {
        let link = format!("{}/reset?token={token}", state.base_url);
        let sent = state
            .mailer
            .send(Mail {
                to: email,
                subject: "Reset your necropsy password".into(),
                body: mail_text(&[
                    "Open this link to choose a new password. It expires in one hour:",
                    &link,
                    "",
                    "If you did not ask for this, ignore this email. Your password has not changed.",
                ]),
            })
            .await;
        if let Err(e) = sent {
            tracing::error!("could not send reset email: {e}");
        }
    }
    reply
}

fn reset_form(
    state: &AppState,
    viewer: Option<&User>,
    token: &str,
    status: StatusCode,
    error: Option<&str>,
) -> Response {
    form_page(
        state,
        viewer,
        status,
        "Choose a new password",
        error,
        html! {
            form method="post" action="/reset" {
                input type="hidden" name="token" value=(token);
                (field("New password", "password", "password", "new-password", Some("At least 10 characters.")))
                button.button type="submit" { "Set password" }
            }
        },
    )
}

pub async fn reset_get(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    Query(q): Query<TokenQuery>,
) -> Response {
    let v = viewer.as_ref();
    let token = q.token.unwrap_or_default();
    let live = token.len() == 64
        && matches!(
            state.db.token_is_live(token_hash(&token), "reset").await,
            Ok(true)
        );
    if !live {
        return notice_page(
            &state,
            v,
            StatusCode::BAD_REQUEST,
            "Link expired",
            "This reset link is invalid or has expired.",
            Some(("/forgot", "Send a new link")),
        );
    }
    reset_form(&state, v, &token, StatusCode::OK, None)
}

#[derive(Deserialize)]
pub struct ResetForm {
    token: String,
    password: String,
}

pub async fn reset_post(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    Form(f): Form<ResetForm>,
) -> Response {
    let v = viewer.as_ref();
    if let Err(e) = validate_password(&f.password) {
        return reset_form(&state, v, &f.token, StatusCode::BAD_REQUEST, Some(e));
    }
    let Ok(hash) = hash_password(f.password).await else {
        return server_error(&state, v);
    };
    let taken = if f.token.len() == 64 {
        state.db.take_token(token_hash(&f.token), "reset").await
    } else {
        Ok(None)
    };
    match taken {
        Ok(Some((user_id, _))) => {
            if state.db.set_password(user_id, hash).await.is_err() {
                return server_error(&state, v);
            }
            notify(
                &state,
                user_id,
                "Your necropsy password was changed",
                mail_text(&["Your password was just reset. If this was not you, reset it again."]),
            )
            .await;
            notice_page(
                &state,
                v,
                StatusCode::OK,
                "Password updated",
                "Log in with your new password.",
                Some(("/login", "Log in")),
            )
        }
        Ok(None) => notice_page(
            &state,
            v,
            StatusCode::BAD_REQUEST,
            "Link expired",
            "This reset link is invalid or has expired.",
            Some(("/forgot", "Send a new link")),
        ),
        Err(e) => {
            tracing::error!("reset failed: {e}");
            server_error(&state, v)
        }
    }
}

// ---------------------------------------------------------------- settings

pub async fn account_page(
    state: &AppState,
    user: &User,
    flash: Option<(bool, &str)>,
    status: StatusCode,
) -> Response {
    let solved = state
        .db
        .solves_of(user.id)
        .await
        .map(|s| s.len())
        .unwrap_or(0);
    let body = html! {
        section.wrap.page-head { h1 { "Account" } }
        section.wrap.section-tight {
            div.auth.account {
                @if let Some((ok, text)) = flash {
                    p.(if ok { "form-ok" } else { "form-error" }) role="status" { (text) }
                }
                dl.facts {
                    div { dt { "Username" } dd { (user.username) } }
                    div { dt { "Joined" } dd { (ymd(user.created_at)) } }
                    div { dt { "Solved" } dd { (solved) " of " (state.visible_count()) } }
                }
                (crate::leaderboard::attempts_section(state, user).await)

                @if user.is_admin {
                    p.auth-links { a href="/admin" { "Admin" } }

                    h2 { "Change username" }
                    form method="post" action="/account/username" {
                        (field("New username", "username", "text", "username",
                            Some("3 to 24 letters, numbers, - and _.")))
                        (field("Password", "password", "password", "current-password", None))
                        button.button.button-quiet type="submit" { "Change username" }
                    }
                }

                h2 { "Change password" }
                form method="post" action="/account/password" {
                    (field("Current password", "current", "password", "current-password", None))
                    (field("New password", "password", "password", "new-password", Some("At least 10 characters.")))
                    button.button.button-quiet type="submit" { "Change password" }
                }

                @if state.mailer.enabled() {
                    h2 { "Change email" }
                    form method="post" action="/account/email" {
                        (field("New email", "email", "email", "email",
                            Some("We send a confirmation link to the new address. It is never shown to anyone.")))
                        (field("Password", "password", "password", "current-password", None))
                        button.button.button-quiet type="submit" { "Change email" }
                    }
                }

                (crate::mfa::settings_section(state, user).await)

                form.logout method="post" action="/logout" {
                    button.button.button-quiet type="submit" { "Log out" }
                }

                @if !user.is_admin {
                    h2 { "Delete account" }
                    form method="post" action="/account/delete" {
                        (field("Password", "password", "password", "current-password",
                            Some("This removes your account and progress.")))
                        button.button.button-danger type="submit" { "Delete account" }
                    }
                }
            }
        }
    };
    (
        status,
        layout(
            state,
            Some(user),
            "Account",
            "Your account.",
            Nav::Account,
            "/account",
            body,
        ),
    )
        .into_response()
}

pub async fn account_get(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
) -> Response {
    match viewer {
        Some(u) => account_page(&state, &u, None, StatusCode::OK).await,
        None => Redirect::to("/login").into_response(),
    }
}

/// Re-checks the current password for settings changes, with its own limit
/// so a stolen session cannot be used to guess the password.
pub(crate) async fn confirm_password(
    state: &AppState,
    user: &User,
    password: String,
) -> Result<(), Box<Response>> {
    if !state
        .limiter
        .allow(&format!("confirm:{}", user.id), 10, AUTH_WINDOW)
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
    let hash = state.db.password_hash_of(user.id).await.ok().flatten();
    let ok = match hash {
        Some(h) => verify_password(password, h).await,
        None => false,
    };
    match ok {
        true => Ok(()),
        false => Err(Box::new(
            account_page(
                state,
                user,
                Some((false, "Wrong password.")),
                StatusCode::FORBIDDEN,
            )
            .await,
        )),
    }
}

#[derive(Deserialize)]
pub struct PasswordChangeForm {
    current: String,
    password: String,
}

pub async fn account_password_post(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    Form(f): Form<PasswordChangeForm>,
) -> Response {
    let Some(user) = viewer else {
        return Redirect::to("/login").into_response();
    };
    if let Err(r) = confirm_password(&state, &user, f.current).await {
        return *r;
    }
    if let Err(e) = validate_password(&f.password) {
        return account_page(&state, &user, Some((false, e)), StatusCode::BAD_REQUEST).await;
    }
    let Ok(hash) = hash_password(f.password).await else {
        return server_error(&state, Some(&user));
    };
    // Changing the password signs out every session, this one included, so
    // start a fresh one to keep the person signed in on this device.
    if state.db.set_password(user.id, hash).await.is_err() {
        return server_error(&state, Some(&user));
    }
    notify(
        &state,
        user.id,
        "Your necropsy password was changed",
        mail_text(&["Your password was just changed. If this was not you, reset it now."]),
    )
    .await;
    match start_session(&state, user.id).await {
        Ok(cookie) => {
            let mut res = account_page(
                &state,
                &user,
                Some((true, "Password changed. Other devices were signed out.")),
                StatusCode::OK,
            )
            .await;
            if let Ok(v) = cookie.parse() {
                res.headers_mut().insert(header::SET_COOKIE, v);
            }
            res
        }
        Err(()) => Redirect::to("/login").into_response(),
    }
}

#[derive(Deserialize)]
pub struct UsernameForm {
    username: String,
    password: String,
}

/// Admin only. The generated `admin-xxxxxx` name is replaced with one the
/// admin chooses, and unlike registration this allows reserved names.
pub async fn account_username_post(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    Form(f): Form<UsernameForm>,
) -> Response {
    let Some(mut user) = viewer else {
        return Redirect::to("/login").into_response();
    };
    if !user.is_admin {
        return StatusCode::NOT_FOUND.into_response();
    }
    if let Err(r) = confirm_password(&state, &user, f.password).await {
        return *r;
    }
    let name = f.username.trim().to_owned();
    if let Err(e) = validate_username_format(&name) {
        return account_page(&state, &user, Some((false, e)), StatusCode::BAD_REQUEST).await;
    }
    match state.db.set_username(user.id, name.clone()).await {
        Ok(()) => {
            user.username = name;
            account_page(
                &state,
                &user,
                Some((true, "Username changed.")),
                StatusCode::OK,
            )
            .await
        }
        Err(CreateError::UsernameTaken) => {
            account_page(
                &state,
                &user,
                Some((false, "That username is taken.")),
                StatusCode::CONFLICT,
            )
            .await
        }
        Err(e) => {
            tracing::error!("username change failed: {e:?}");
            server_error(&state, Some(&user))
        }
    }
}

#[derive(Deserialize)]
pub struct EmailChangeForm {
    email: String,
    password: String,
}

pub async fn account_email_post(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    Form(f): Form<EmailChangeForm>,
) -> Response {
    let Some(user) = viewer else {
        return Redirect::to("/login").into_response();
    };
    if !state.mailer.enabled() {
        return StatusCode::NOT_FOUND.into_response();
    }
    if let Err(r) = confirm_password(&state, &user, f.password).await {
        return *r;
    }
    let Some(email) = normalize_email(&f.email) else {
        return account_page(
            &state,
            &user,
            Some((false, "Enter a valid email address.")),
            StatusCode::BAD_REQUEST,
        )
        .await;
    };
    let Ok(token) = issue_token(
        &state,
        user.id,
        "change_email",
        Some(email.clone()),
        CHANGE_EMAIL_TTL_SECS,
    )
    .await
    else {
        return server_error(&state, Some(&user));
    };
    let link = format!("{}/email/confirm?token={token}", state.base_url);
    let sent = state
        .mailer
        .send(Mail {
            to: email,
            subject: "Confirm your new necropsy email".into(),
            body: mail_text(&[
                "Open this link to use this address for your necropsy account. It expires in 24 hours:",
                &link,
                "",
                "If you did not ask for this, ignore this email.",
            ]),
        })
        .await;
    match sent {
        Ok(()) => {
            account_page(
                &state,
                &user,
                Some((true, "We sent a confirmation link to the new address.")),
                StatusCode::OK,
            )
            .await
        }
        Err(e) => {
            tracing::error!("could not send email-change confirmation: {e}");
            account_page(
                &state,
                &user,
                Some((false, "Could not send the confirmation email.")),
                StatusCode::BAD_GATEWAY,
            )
            .await
        }
    }
}

pub async fn email_confirm_get(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    Query(q): Query<TokenQuery>,
) -> Response {
    let v = viewer.as_ref();
    let bad = |text: &str| {
        notice_page(
            &state,
            v,
            StatusCode::BAD_REQUEST,
            "Link expired",
            text,
            None,
        )
    };
    let Some(token) = q.token.filter(|t| t.len() == 64) else {
        return bad("This confirmation link is invalid or has expired.");
    };
    match state
        .db
        .take_token(token_hash(&token), "change_email")
        .await
    {
        Ok(Some((user_id, Some(email)))) => {
            // Tell the old address first; once the swap happens it is gone.
            notify(
                &state,
                user_id,
                "Your necropsy email was changed",
                mail_text(&["The email address on your account was just changed. If this was not you, contact the site owner."]),
            )
            .await;
            match state.db.set_email(user_id, email).await {
                Ok(()) => notice_page(
                    &state,
                    v,
                    StatusCode::OK,
                    "Email updated",
                    "Your new address is confirmed.",
                    Some(("/account", "Account")),
                ),
                Err(CreateError::EmailTaken) => {
                    bad("That address is already used by another account.")
                }
                Err(e) => {
                    tracing::error!("email change failed: {e:?}");
                    server_error(&state, v)
                }
            }
        }
        Ok(_) => bad("This confirmation link is invalid or has expired."),
        Err(e) => {
            tracing::error!("email confirm failed: {e}");
            server_error(&state, v)
        }
    }
}

#[derive(Deserialize)]
pub struct DeleteForm {
    password: String,
}

pub async fn account_delete_post(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    Form(f): Form<DeleteForm>,
) -> Response {
    let Some(user) = viewer else {
        return Redirect::to("/login").into_response();
    };
    // The admin account is the only way into /admin, so it is not
    // self-service deletable.
    if user.is_admin {
        return StatusCode::NOT_FOUND.into_response();
    }
    if let Err(r) = confirm_password(&state, &user, f.password).await {
        return *r;
    }
    if state.db.delete_user(user.id).await.is_err() {
        return server_error(&state, Some(&user));
    }
    (
        [(header::SET_COOKIE, clear_cookie(state.secure_cookies))],
        Redirect::to("/"),
    )
        .into_response()
}

/// How often the housekeeping task runs.
pub const PURGE_INTERVAL: Duration = Duration::from_secs(3600);
