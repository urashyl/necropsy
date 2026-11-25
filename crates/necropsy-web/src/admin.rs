// SPDX-License-Identifier: AGPL-3.0-or-later
//! Admin page: user list with disable, enable and delete, and email status
//! with a test send. Only the generated admin account can reach it (see
//! `AdminUser`). It lists usernames, never email addresses, because no code
//! path reads them out.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::Form;
use maud::{html, Markup};
use serde::Deserialize;

use crate::accounts::format_date;
use crate::auth::{normalize_email, AdminUser};
use crate::db::{AdminRow, Instance};
use crate::live;
use crate::mail::Mail;
use crate::pages::{layout, Nav};
use crate::state::AppState;

async fn admin_page(
    state: &AppState,
    admin: &crate::db::User,
    flash: Option<(bool, String)>,
    status: StatusCode,
) -> Response {
    let rows = match state.db.list_users().await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("listing users failed: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let total = state.visible_count();
    // Every loaded live puzzle, with the admin's running instance of it, if any.
    let mut live_rows = Vec::new();
    for p in state.all_live() {
        let inst = state
            .db
            .live_active(admin.id, p.pack.meta.id.clone())
            .await
            .ok()
            .flatten();
        live_rows.push((p, inst));
    }
    // Whether the range can be reached, from here, with the token.
    let range_status = match state.live.as_ref().and_then(|l| l.range.as_ref()) {
        Some(r) => Some((r.url.clone(), r.check().await)),
        None => None,
    };
    let body: Markup = html! {
        section.wrap.page-head { h1 { "Admin" } }
        section.wrap.section-tight {
            @if let Some((ok, text)) = &flash {
                p.(if *ok { "form-ok" } else { "form-error" }) role="status" { (text) }
            }

            h2.admin-h { "Email" }
            dl.facts.admin-facts {
                div { dt { "Sending" } dd { (state.mailer.describe()) } }
            }
            @if state.mailer.enabled() {
                form.inline-form method="post" action="/admin/test-email" {
                    label for="to" { "Send a test email to" }
                    div.answer-row {
                        input #to name="to" type="email" required autocomplete="off" maxlength="254";
                        button.button.button-quiet type="submit" { "Send test" }
                    }
                }
            }

            h2.admin-h { "Live puzzles" }
            @if state.live.is_none() {
                p.muted { "Live puzzles are off. Set NECROPSY_LIVE_DOMAIN to turn them on." }
            } @else if live_rows.is_empty() {
                p.muted { "No live puzzles are loaded." }
            } @else {
                @match &range_status {
                    Some((url, Ok(()))) => {
                        p.form-ok { "Range " (url) ": reachable, and it accepts this site's token." }
                    }
                    Some((_, Err(why))) => {
                        p.form-error { "Range: " (why) }
                    }
                    None => {
                        p.muted { "No range is set (NECROPSY_RANGE_URL), so puzzles that need one are not loaded." }
                    }
                }
                p.muted {
                    "Start one to try it, whatever its state. It is yours alone, with no captcha and no "
                    "limit, and its flag is shown here. Puzzles that need the range are loaded only "
                    "when NECROPSY_RANGE_URL is set."
                }
                div.table-wrap {
                    table.table {
                        thead { tr { th { "Puzzle" } th { "Kind" } th { "Your instance" } } }
                        tbody { @for (p, inst) in &live_rows { (live_row(state, p, inst.as_ref())) } }
                    }
                }
            }

            h2.admin-h { "Users" }
            @if rows.len() <= 1 {
                p.muted { "No other accounts yet." }
            }
            div.table-wrap {
                table.table {
                    thead { tr { th { "Username" } th { "Joined" } th { "Status" } th { "Solved" } th {} } }
                    tbody { @for r in &rows { (user_row(r, total)) } }
                }
            }
        }
    };
    (
        status,
        layout(
            state,
            Some(admin),
            "Admin",
            "Administration.",
            Nav::Admin,
            "/admin",
            body,
        ),
    )
        .into_response()
}

fn user_row(r: &AdminRow, total: usize) -> Markup {
    let u = &r.user;
    let status = if u.is_admin {
        "Admin"
    } else if u.disabled {
        "Disabled"
    } else if !u.verified {
        "Unverified"
    } else {
        "Active"
    };
    html! {
        tr {
            td { (u.username) }
            td.tabular { (format_date(u.created_at)) }
            td { (status) }
            td.tabular { (r.solves) "/" (total) }
            td {
                div.row-actions { @if !u.is_admin {
                    form method="post" action={"/admin/users/" (u.id) "/" (if u.disabled { "enable" } else { "disable" })} {
                        button.link-button type="submit" { (if u.disabled { "Enable" } else { "Disable" }) }
                    }
                    a.link-button href={"/admin/users/" (u.id) "/delete"} { "Delete" }
                } }
            }
        }
    }
}

pub async fn admin_get(
    State(state): State<Arc<AppState>>,
    AdminUser(admin): AdminUser,
) -> Response {
    admin_page(&state, &admin, None, StatusCode::OK).await
}

fn live_row(state: &AppState, p: &crate::state::RenderedPack, inst: Option<&Instance>) -> Markup {
    let m = &p.pack.meta;
    let kind = if p.pack.container().is_some() {
        "a lab on the range"
    } else {
        "simulated"
    };
    html! {
        tr {
            td {
                a href={"/p/" (m.id)} { (m.title) }
                " " span.muted { (p.label()) " · " (m.id) }
            }
            td { (kind) }
            td {
                @if let Some(i) = inst {
                    @let url = live::instance_url(state, &i.label).unwrap_or_default();
                    @let flag = p.pack.live.as_ref().map(|s| live::flag_of(s, &m.id, &i.seed)).unwrap_or_default();
                    a href=(url) target="_blank" rel="noopener noreferrer" { (url) }
                    br;
                    span.muted { ((i.expires_at - crate::db::now()).max(0) / 60) " min left · flag " }
                    code { (flag) }
                    form.inline-form method="post" action={"/admin/live/" (m.id) "/stop"} {
                        button.button.button-quiet type="submit" { "Stop" }
                    }
                } @else {
                    form.inline-form method="post" action={"/admin/live/" (m.id) "/start"} {
                        button.button type="submit" { "Start an instance" }
                    }
                }
            }
        }
    }
}

/// Starts the admin's own instance of a live puzzle, whether or not it is
/// released: no captcha, no limit, and the flag is shown on the admin page.
pub async fn live_start_post(
    State(state): State<Arc<AppState>>,
    AdminUser(admin): AdminUser,
    Path(id): Path<String>,
) -> Response {
    let Some(rp) = state.get_any(&id).filter(|p| p.pack.is_live()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if state.live.is_none() {
        return StatusCode::NOT_FOUND.into_response();
    }
    match state.db.live_active(admin.id, id.clone()).await {
        Ok(Some(_)) => return Redirect::to("/admin").into_response(),
        Ok(None) => {}
        Err(e) => {
            tracing::error!("admin live lookup failed: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }
    match live::create_instance(&state, admin.id, &rp.pack).await {
        Ok(_) => Redirect::to("/admin").into_response(),
        Err(refusal) => refusal.into_response(),
    }
}

/// Stops the admin's instance of a live puzzle.
pub async fn live_stop_post(
    State(state): State<Arc<AppState>>,
    AdminUser(admin): AdminUser,
    Path(id): Path<String>,
) -> Response {
    let Some(rp) = state.get_any(&id).filter(|p| p.pack.is_live()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    live::stop_instances(&state, admin.id, &rp.pack).await;
    Redirect::to("/admin").into_response()
}

/// `enable` or `disable`. Admin accounts are never touched.
pub async fn user_action_post(
    State(state): State<Arc<AppState>>,
    AdminUser(_admin): AdminUser,
    Path((id, action)): Path<(i64, String)>,
) -> Response {
    let disable = match action.as_str() {
        "disable" => true,
        "enable" => false,
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    match state.db.user_by_id(id).await {
        Ok(Some(u)) if !u.is_admin => {
            if state.db.set_disabled(id, disable).await.is_err() {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            Redirect::to("/admin").into_response()
        }
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Confirmation page, so a stray click cannot delete an account.
pub async fn delete_confirm_get(
    State(state): State<Arc<AppState>>,
    AdminUser(admin): AdminUser,
    Path(id): Path<i64>,
) -> Response {
    let Ok(Some(u)) = state.db.user_by_id(id).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if u.is_admin {
        return StatusCode::NOT_FOUND.into_response();
    }
    let body = html! {
        section.wrap.page-head { h1 { "Delete " (u.username) "?" } }
        section.wrap.section-tight {
            div.auth {
                p { "This removes the account and its progress. It cannot be undone." }
                form method="post" action={"/admin/users/" (u.id) "/delete"} {
                    button.button.button-danger type="submit" { "Delete account" }
                    " "
                    a.button.button-quiet href="/admin" { "Cancel" }
                }
            }
        }
    };
    layout(
        &state,
        Some(&admin),
        "Delete account",
        "Delete an account.",
        Nav::Admin,
        "/admin",
        body,
    )
    .into_response()
}

pub async fn delete_post(
    State(state): State<Arc<AppState>>,
    AdminUser(_admin): AdminUser,
    Path(id): Path<i64>,
) -> Response {
    match state.db.user_by_id(id).await {
        Ok(Some(u)) if !u.is_admin => {
            if state.db.delete_user(id).await.is_err() {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            Redirect::to("/admin").into_response()
        }
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[derive(Deserialize)]
pub struct TestEmailForm {
    to: String,
}

pub async fn test_email_post(
    State(state): State<Arc<AppState>>,
    AdminUser(admin): AdminUser,
    Form(f): Form<TestEmailForm>,
) -> Response {
    let Some(to) = normalize_email(&f.to) else {
        return admin_page(
            &state,
            &admin,
            Some((false, "Enter a valid email address.".into())),
            StatusCode::BAD_REQUEST,
        )
        .await;
    };
    let sent = state
        .mailer
        .send(Mail {
            to,
            subject: "necropsy test email".into(),
            body: "Email from necropsy is working.\n".into(),
        })
        .await;
    match sent {
        Ok(()) => {
            admin_page(
                &state,
                &admin,
                Some((true, "Test email sent.".into())),
                StatusCode::OK,
            )
            .await
        }
        Err(e) => {
            admin_page(
                &state,
                &admin,
                Some((false, format!("Sending failed: {e}"))),
                StatusCode::BAD_GATEWAY,
            )
            .await
        }
    }
}
