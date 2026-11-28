// SPDX-License-Identifier: AGPL-3.0-or-later
//! The two leaderboards (docs/leaderboards.md).
//!
//! - Casual, `/leaderboard`: streaks, puzzles solved and Learn badges, from the
//!   solves already recorded. Nothing is checked beyond what a solve already is.
//! - Ranked, `/leaderboard/ranked`: live dailies only, from each player's one
//!   ranked attempt (see `db::RANKED_SCHEMA`). An admin can hide an entry, with
//!   a reason; its time is never changed.

use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::{Form, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use maud::{html, Markup};
use necropsy_pack::Track;
use serde::Deserialize;

use crate::auth::{AdminUser, MaybeUser};
use crate::cards::streak_ending;
use crate::pages::{clock, layout, Nav};
use crate::state::{AppState, RenderedPack};

/// Players shown on the casual board.
pub const CASUAL_SIZE: usize = 100;

/// One player on the casual board.
#[derive(Debug, PartialEq, Eq)]
pub struct CasualEntry {
    pub username: String,
    pub streak: i64,
    pub solved: i64,
    pub badges: i64,
}

/// The casual board: current streak (dailies solved on their own day, ending
/// today or yesterday), then puzzles solved, then Learn badges.
pub fn casual_entries(state: &AppState, rows: Vec<crate::db::CasualRow>) -> Vec<CasualEntry> {
    let today = state.today_days();
    let mut out: Vec<CasualEntry> = rows
        .into_iter()
        .map(|r| {
            let days: HashSet<i64> = r
                .daily_solves
                .iter()
                .filter_map(|(id, at, tz)| {
                    let day = (at + tz * 60).div_euclid(86_400);
                    state
                        .get_any(id)
                        .filter(|p| p.pack.meta.release_day() == Some(day))
                        .map(|_| day)
                })
                .collect();
            let end = if days.contains(&today) {
                today
            } else {
                today - 1
            };
            CasualEntry {
                username: r.username,
                streak: streak_ending(&days, end),
                solved: r.solved,
                badges: r.badges,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        (b.streak, b.solved, b.badges)
            .cmp(&(a.streak, a.solved, a.badges))
            .then_with(|| a.username.to_lowercase().cmp(&b.username.to_lowercase()))
    });
    out.truncate(CASUAL_SIZE);
    out
}

fn tabs(current: &str) -> Markup {
    html! {
        nav.board-tabs aria-label="Leaderboards" {
            a href="/leaderboard" aria-current=[(current == "casual").then_some("page")] { "Casual" }
            a href="/leaderboard/ranked" aria-current=[(current == "ranked").then_some("page")] { "Ranked" }
        }
    }
}

/// `GET /leaderboard`.
pub async fn casual(State(state): State<Arc<AppState>>, MaybeUser(viewer): MaybeUser) -> Response {
    let rows = match state.db.casual_rows().await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("casual board failed: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let entries = casual_entries(&state, rows);
    let body = html! {
        section.wrap.page-head {
            h1 { "Leaderboard" }
            (tabs("casual"))
            p.page-lead { "Streaks, puzzles solved and Learn badges. A streak counts dailies solved on their own day, ending today or yesterday." }
        }
        section.wrap.section-tight {
            @if entries.is_empty() {
                p.muted { "No solves yet." }
            } @else {
                div.table-wrap {
                    table.table.board {
                        thead { tr { th { "#" } th { "Player" } th.num { "Streak" } th.num { "Solved" } th.num { "Badges" } } }
                        tbody {
                            @for (i, e) in entries.iter().enumerate() {
                                tr {
                                    td.num { (i + 1) }
                                    td.player { (e.username) }
                                    td.num { (e.streak) }
                                    td.num { (e.solved) }
                                    td.num { (e.badges) }
                                }
                            }
                        }
                    }
                }
            }
        }
    };
    layout(
        &state,
        viewer.as_ref(),
        "Leaderboard",
        "Streaks and puzzles solved on necropsy.",
        Nav::Leaderboard,
        "/leaderboard",
        body,
    )
    .into_response()
}

#[derive(Deserialize)]
pub struct RankedQuery {
    pack: Option<String>,
}

/// Released live dailies, newest first: the puzzles with a ranked board.
pub fn ranked_dailies(state: &AppState) -> Vec<&RenderedPack> {
    let mut list: Vec<&RenderedPack> = state
        .released_dailies()
        .into_iter()
        .filter(|p| p.pack.is_live())
        .collect();
    list.reverse();
    list
}

/// `GET /leaderboard/ranked[?pack=<id>]`: one live daily's board, today's or
/// the latest by default.
pub async fn ranked(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    Query(q): Query<RankedQuery>,
) -> Response {
    let dailies = ranked_dailies(&state);
    let chosen = match &q.pack {
        Some(id) => match dailies.iter().find(|p| &p.pack.meta.id == id) {
            Some(p) => Some(*p),
            None => return StatusCode::NOT_FOUND.into_response(),
        },
        None => dailies.first().copied(),
    };
    let admin = viewer.as_ref().is_some_and(|u| u.is_admin);
    let board = match chosen {
        Some(p) => match state.db.ranked_board(p.pack.meta.id.clone()).await {
            Ok(b) => Some(b),
            Err(e) => {
                tracing::error!("ranked board failed: {e}");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        },
        None => None,
    };
    let body = html! {
        section.wrap.page-head {
            h1 { "Leaderboard" }
            (tabs("ranked"))
            p.page-lead {
                "Live dailies only. Each player gets one ranked attempt per daily: it starts with their first instance on the puzzle's day and ends when they solve it, stop the instance or it runs out. Later attempts count only on the player's own account page."
            }
        }
        section.wrap.section-tight {
            @if let (Some(p), Some((rows, total))) = (chosen, &board) {
                form.board-pick method="get" action="/leaderboard/ranked" {
                    label for="board-pack" { "Daily" }
                    select #board-pack name="pack" {
                        @for d in &dailies {
                            option value=(d.pack.meta.id) selected[d.pack.meta.id == p.pack.meta.id] {
                                (d.label()) " · " (d.pack.meta.title)
                            }
                        }
                    }
                    button.button.button-quiet type="submit" { "Show" }
                }
                @let shown = ranked_rows(rows, admin);
                p.muted {
                    a href={"/p/" (p.pack.meta.id)} { (p.pack.meta.title) } ": "
                    (shown.iter().filter(|(_, r)| !r.hidden).count()) " solved of " (total) " ranked attempts."
                }
                @if shown.is_empty() {
                    p.muted { "Nobody has solved it in a ranked attempt yet." }
                } @else {
                    div.table-wrap {
                        table.table.board {
                            thead { tr {
                                th { "#" } th { "Player" } th.num { "Time" } th.num { "Hints" }
                                @if admin { th { "Admin" } }
                            } }
                            tbody {
                                @for (rank, r) in &shown {
                                    tr.(if r.hidden { "board-hidden" } else { "" }) {
                                        td.num { @if let Some(n) = rank { (n) } @else { "–" } }
                                        td.player { (r.username) }
                                        td.num { (clock(r.seconds as f64)) }
                                        td.num { (r.hints) }
                                        @if admin {
                                            td { (hide_form(&p.pack.meta.id, r)) }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            } @else {
                p.muted { "No live daily has been released yet." }
            }
        }
    };
    layout(
        &state,
        viewer.as_ref(),
        "Ranked leaderboard",
        "Fastest ranked solves of each live daily on necropsy.",
        Nav::Leaderboard,
        "/leaderboard/ranked",
        body,
    )
    .into_response()
}

/// The rows to show with their places: hidden entries take no place, and are
/// shown only to an admin.
pub fn ranked_rows(
    rows: &[crate::db::RankedRow],
    admin: bool,
) -> Vec<(Option<usize>, &crate::db::RankedRow)> {
    let mut place = 0;
    rows.iter()
        .filter(|r| admin || !r.hidden)
        .map(|r| {
            if r.hidden {
                (None, r)
            } else {
                place += 1;
                (Some(place), r)
            }
        })
        .collect()
}

fn hide_form(pack: &str, r: &crate::db::RankedRow) -> Markup {
    let action = format!("/leaderboard/ranked/{pack}/{}/", r.user_id);
    html! {
        @if r.hidden {
            form.inline-form method="post" action={(action) "show"} {
                span.muted { "Hidden: " (r.hidden_reason.as_deref().unwrap_or("")) " " }
                button.link-button type="submit" { "Reinstate" }
            }
        } @else {
            form.inline-form method="post" action={(action) "hide"} {
                label.visually-hidden for={"reason-" (r.user_id)} { "Reason" }
                input id={"reason-" (r.user_id)} name="reason" type="text" required maxlength="200" placeholder="Reason";
                button.link-button type="submit" { "Hide" }
            }
        }
    }
}

#[derive(Deserialize)]
pub struct HideForm {
    reason: String,
}

/// `POST /leaderboard/ranked/{pack}/{user}/hide`: an admin takes an entry off
/// the board. The attempt itself, its status and its time, are kept.
pub async fn hide(
    State(state): State<Arc<AppState>>,
    AdminUser(admin): AdminUser,
    Path((pack, user)): Path<(String, i64)>,
    Form(f): Form<HideForm>,
) -> Response {
    let reason = f.reason.trim();
    if reason.is_empty() || reason.chars().count() > 200 {
        return (
            StatusCode::BAD_REQUEST,
            "Give a reason, up to 200 characters.",
        )
            .into_response();
    }
    set_hidden(&state, pack, user, admin.id, Some(reason.to_owned())).await
}

/// `POST /leaderboard/ranked/{pack}/{user}/show`: puts a hidden entry back.
pub async fn show(
    State(state): State<Arc<AppState>>,
    AdminUser(admin): AdminUser,
    Path((pack, user)): Path<(String, i64)>,
) -> Response {
    set_hidden(&state, pack, user, admin.id, None).await
}

async fn set_hidden(
    state: &AppState,
    pack: String,
    user: i64,
    by: i64,
    reason: Option<String>,
) -> Response {
    let back = format!("/leaderboard/ranked?pack={pack}");
    match state
        .db
        .ranked_hide(user, pack.clone(), by, reason.clone())
        .await
    {
        Ok(true) => {
            tracing::info!(
                "admin {by} {} the ranked attempt of user {user} at {pack}{}",
                if reason.is_some() {
                    "hid"
                } else {
                    "reinstated"
                },
                reason.map(|r| format!(": {r}")).unwrap_or_default()
            );
            Redirect::to(&back).into_response()
        }
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!("hiding a ranked attempt failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// The viewer's ranked attempt at a live daily: running, solved or failed.
pub async fn ranked_status_for(
    state: &AppState,
    viewer: Option<&crate::db::User>,
    rp: &RenderedPack,
) -> Option<String> {
    let user = viewer?;
    if rp.pack.meta.track != Track::Daily || !rp.pack.is_live() {
        return None;
    }
    match state
        .db
        .ranked_status(user.id, rp.pack.meta.id.clone())
        .await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("ranked status failed: {e}");
            None
        }
    }
}

/// The ranked-attempt notice on a live daily's page: what starting it means.
pub fn live_notice(state: &AppState, rp: &RenderedPack, status: Option<&str>) -> Markup {
    if rp.pack.meta.track != Track::Daily || !rp.pack.is_live() {
        return html! {};
    }
    let state_attr = match (state.is_today(rp), status) {
        (true, None) => "new",
        (_, Some(s)) => s,
        (false, None) => "off-day",
    };
    html! {
        @match (state.is_today(rp), status) {
            (true, None) => p.notice.ranked-notice data-state=(state_attr) {
                "Starting the instance starts your ranked attempt. You get one: it ends when you solve it, stop the instance or it runs out. "
                a href="/leaderboard/ranked" { "Ranked board" }
            },
            (_, Some("running")) => p.muted.ranked-notice data-state=(state_attr) { "Your ranked attempt is running." },
            (_, Some("solved")) => p.muted.ranked-notice data-state=(state_attr) { "Your ranked attempt is done. Further attempts are on your account page only." },
            (_, Some(_)) => p.muted.ranked-notice data-state=(state_attr) { "Your ranked attempt has ended. Further attempts are on your account page only." },
            (false, None) => p.muted.ranked-notice data-state=(state_attr) { "Ranked attempts are only made on the puzzle's day. This attempt is on your account page only." },
        }
    }
}

/// The player's own attempts at live dailies, on their account page: the
/// ranked one and their personal records, which appear nowhere else.
pub async fn attempts_section(state: &AppState, user: &crate::db::User) -> Markup {
    let rows = match state.db.attempts_of(user.id).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("loading attempts failed: {e}");
            return html! {};
        }
    };
    if rows.is_empty() {
        return html! {};
    }
    html! {
        h2 { "Live daily attempts" }
        p.muted { "Only you see this list. Only ranked attempts are on the " a href="/leaderboard/ranked" { "ranked board" } "." }
        div.table-wrap {
            table.table.board {
                thead { tr { th { "Puzzle" } th { "Kind" } th { "Result" } th.num { "Time" } } }
                tbody {
                    @for a in &rows {
                        tr {
                            td {
                                @match state.get_any(&a.pack_id) {
                                    Some(p) => a href={"/p/" (a.pack_id)} { (p.pack.meta.title) },
                                    None => code { (a.pack_id) },
                                }
                            }
                            td {
                                @if a.kind == crate::db::Attempt::Ranked {
                                    "Ranked" @if a.hidden { " (hidden by an admin)" }
                                } @else { "Personal" }
                            }
                            td { (match a.status.as_str() { "solved" => "Solved", "running" => "Running", _ => "Ended" }) }
                            td.num { @if let Some(sec) = a.seconds { (clock(sec as f64)) } @else { "–" } }
                        }
                    }
                }
            }
        }
    }
}
