// SPDX-License-Identifier: AGPL-3.0-or-later
//! A signed-in player's time and hints on a puzzle, kept by the server so the browser
//! cannot report them. See `docs/web.md`.
//!
//! The page sends a beat every few seconds while it is open. The time since the last
//! beat is added to the player's active time, but at most [`GAP_CAP`] seconds of it, so
//! a closed tab or a sleeping machine does not count while a background tab, whose
//! timers are slowed down but not stopped, still does. Hints are not in the page: each
//! one is sent when the player asks for it, and counted then. When the answer is right,
//! the solve is recorded with these numbers, whatever the browser says.
//!
//! Signed-out players keep their own time and hint count in the browser (the server
//! stores nothing about them), but hint text still comes from the server on request.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::auth::MaybeUser;
use crate::state::AppState;

/// The most seconds one gap between beats adds to the active time.
pub const GAP_CAP: i64 = 120;

#[derive(Deserialize)]
pub struct HaveReq {
    /// How many hints the browser is showing. Only used when signed out.
    #[serde(default)]
    have: i64,
    /// Only read the hints, as a signed-out player would: start no clock. For a
    /// puzzle this browser already solved, before the solve is uploaded.
    #[serde(default)]
    peek: bool,
}

#[derive(Serialize)]
pub struct StateRes {
    /// Whether the server is keeping this player's time and hints.
    user: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    seconds: Option<i64>,
    hints: i64,
    /// The hints revealed so far, in order.
    texts: Vec<String>,
    solved: bool,
}

fn first(hints: &[String], n: i64) -> Vec<String> {
    hints.iter().take(n.max(0) as usize).cloned().collect()
}

async fn respond(
    state: &AppState,
    viewer: Option<crate::db::User>,
    id: &str,
    have: i64,
    reveal: bool,
    peek: bool,
) -> Response {
    let Some(p) = state.get(id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let hints = &p.pack.meta.hints;
    let total = hints.len() as i64;
    let user = if peek { None } else { viewer };
    let Some(user) = user else {
        let n = (have + i64::from(reveal)).clamp(0, total);
        return Json(StateRes {
            user: false,
            seconds: None,
            hints: n,
            texts: first(hints, n),
            solved: false,
        })
        .into_response();
    };
    if !state
        .limiter
        .allow(&format!("beat:{}", user.id), 120, Duration::from_secs(60))
    {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    // A solved puzzle shows what the solve recorded and counts nothing more.
    if let Ok(Some(s)) = state.db.solve_of(user.id, id.to_owned()).await {
        return Json(StateRes {
            user: true,
            seconds: Some(s.seconds),
            hints: s.hints,
            texts: first(hints, s.hints),
            solved: true,
        })
        .into_response();
    }
    let result = if reveal {
        state
            .db
            .progress_hint(user.id, id.to_owned(), total, GAP_CAP)
            .await
    } else {
        state
            .db
            .progress_beat(user.id, id.to_owned(), GAP_CAP)
            .await
    };
    match result {
        Ok(pr) => Json(StateRes {
            user: true,
            seconds: Some(pr.seconds),
            hints: pr.hints,
            texts: first(hints, pr.hints),
            solved: false,
        })
        .into_response(),
        Err(e) => {
            tracing::error!("progress failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// The page is open: count the time, and return the hints shown so far.
pub async fn beat(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    Path(id): Path<String>,
    Json(req): Json<HaveReq>,
) -> Response {
    respond(&state, viewer, &id, req.have, false, req.peek).await
}

/// Reveals the next hint.
pub async fn hint(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    Path(id): Path<String>,
    Json(req): Json<HaveReq>,
) -> Response {
    respond(&state, viewer, &id, req.have, true, req.peek).await
}
