// SPDX-License-Identifier: AGPL-3.0-or-later
//! Moderation (docs/moderation.md): proofs from the top of each ranked board,
//! the moderator role, and the audit log.
//!
//! - The top N eligible entries of a live daily's ranked board are each asked
//!   for a proof: an explanation of the solve, written within 90 seconds of
//!   starting it. A proof not started within a day, or not sent in time, is
//!   forfeited. A forfeited or rejected entry leaves the board, and whoever
//!   moves up into the top N is asked in turn.
//! - Moderators (and admins) review proofs and remove or reinstate entries.
//!   Admins name the moderators.
//! - Every review, removal, reinstatement, forfeit and role change is written
//!   to `audit_log` in the same transaction as the change itself.

use std::sync::Arc;

use axum::extract::{Form, FromRequestParts, Path, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use maud::{html, Markup};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;

use crate::auth::{AdminUser, MaybeUser};
use crate::db::{now, Db, User};
use crate::pages::{clock, layout, Nav};
use crate::state::AppState;

/// Seconds to write a proof, from pressing Start.
pub const PROOF_SECONDS: i64 = 90;
/// Allowance on the deadline for the time a submission takes to arrive.
pub const PROOF_GRACE: i64 = 2;
/// Seconds a player has to start a proof once it is asked for.
pub const PROOF_OPEN_WINDOW: i64 = 24 * 3600;
/// Shortest and longest proof.
pub const PROOF_MIN_CHARS: usize = 20;
pub const PROOF_MAX_CHARS: usize = 2000;
/// Longest reason a moderator gives.
pub const REASON_MAX_CHARS: usize = 200;
/// Default size of the top of a board that must prove its solves.
pub const DEFAULT_TOP_N: usize = 10;

/// A proof, as its player and the moderators see it.
#[derive(Clone, Debug)]
pub struct Proof {
    pub id: i64,
    pub user_id: i64,
    pub pack_id: String,
    pub offered_at: i64,
    pub opened_at: Option<i64>,
    pub deadline: Option<i64>,
    pub text: Option<String>,
    pub status: String,
    pub reason: Option<String>,
}

/// A proof waiting for review, with what the moderator needs to judge it.
#[derive(Clone, Debug)]
pub struct PendingProof {
    pub id: i64,
    pub username: String,
    pub pack_id: String,
    pub seconds: i64,
    pub hints: i64,
    pub text: String,
    pub submitted_at: i64,
}

#[derive(Clone, Debug)]
pub struct AuditEntry {
    pub at: i64,
    pub actor: String,
    pub action: String,
    pub target: Option<String>,
    pub pack_id: Option<String>,
    pub reason: Option<String>,
}

const PROOF_COLS: &str =
    "id, user_id, pack_id, offered_at, opened_at, deadline, text, status, reason";

fn row_proof(r: &rusqlite::Row<'_>) -> rusqlite::Result<Proof> {
    Ok(Proof {
        id: r.get(0)?,
        user_id: r.get(1)?,
        pack_id: r.get(2)?,
        offered_at: r.get(3)?,
        opened_at: r.get(4)?,
        deadline: r.get(5)?,
        text: r.get(6)?,
        status: r.get(7)?,
        reason: r.get(8)?,
    })
}

/// Who did something, for the audit log: a user, or the site itself.
#[derive(Clone, Copy)]
pub enum Actor<'a> {
    User(&'a User),
    System,
}

#[allow(clippy::too_many_arguments)]
fn audit(
    c: &Connection,
    actor: Actor<'_>,
    action: &str,
    target_id: Option<i64>,
    pack_id: Option<&str>,
    reason: Option<&str>,
) -> rusqlite::Result<()> {
    let (actor_id, actor_name) = match actor {
        Actor::User(u) => (Some(u.id), u.username.clone()),
        Actor::System => (None, "the site".to_owned()),
    };
    let target: Option<String> = match target_id {
        Some(id) => c
            .query_row("SELECT username FROM users WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .optional()?,
        None => None,
    };
    c.execute(
        "INSERT INTO audit_log (at, actor_id, actor, action, target_id, target, pack_id, reason)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            now(),
            actor_id,
            actor_name,
            action,
            target_id,
            target,
            pack_id,
            reason
        ],
    )?;
    Ok(())
}

/// Forfeits proofs not started within [`PROOF_OPEN_WINDOW`], or started and
/// not sent by the deadline (and grace). Each is audited.
fn sweep(c: &Connection) -> rusqlite::Result<usize> {
    let t = now();
    let late: Vec<(i64, i64, String)> = {
        let mut st = c.prepare(
            "SELECT id, user_id, pack_id FROM proofs WHERE status = 'open' AND (
                 (opened_at IS NULL AND ?1 > offered_at + ?2)
              OR (opened_at IS NOT NULL AND ?1 > deadline + ?3))",
        )?;
        let rows = st.query_map(params![t, PROOF_OPEN_WINDOW, PROOF_GRACE], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for (id, user, pack) in &late {
        c.execute(
            "UPDATE proofs SET status = 'forfeited', reviewed_at = ?2 WHERE id = ?1",
            params![id, t],
        )?;
        audit(
            c,
            Actor::System,
            "proof forfeited",
            Some(*user),
            Some(pack),
            None,
        )?;
    }
    Ok(late.len())
}

/// Asks the top `top_n` eligible entries of a pack's board for a proof, if
/// they have not been asked. Eligible: solved, not removed, enabled, and not
/// forfeited or rejected. Returns the players newly asked.
fn ensure(c: &Connection, pack: &str, top_n: usize) -> rusqlite::Result<Vec<i64>> {
    let top: Vec<(i64, bool)> = {
        let mut st = c.prepare(
            "SELECT a.user_id, p.id IS NULL
             FROM ranked_attempts a
             JOIN users u ON u.id = a.user_id
             LEFT JOIN proofs p ON p.user_id = a.user_id AND p.pack_id = a.pack_id
             WHERE a.pack_id = ?1 AND a.status = 'solved' AND a.hidden = 0 AND u.disabled = 0
               AND (p.status IS NULL OR p.status NOT IN ('rejected', 'forfeited'))
             ORDER BY a.seconds, a.ended_at, u.id
             LIMIT ?2",
        )?;
        let rows = st.query_map(params![pack, top_n as i64], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let t = now();
    let mut asked = Vec::new();
    for (user, missing) in top {
        if missing {
            c.execute(
                "INSERT INTO proofs (user_id, pack_id, offered_at, status) VALUES (?1, ?2, ?3, 'open')",
                params![user, pack, t],
            )?;
            asked.push(user);
        }
    }
    Ok(asked)
}

fn check_reason(reason: &str) -> Result<String, &'static str> {
    let r = reason.trim();
    if r.is_empty() || r.chars().count() > REASON_MAX_CHARS {
        return Err("Give a reason, up to 200 characters.");
    }
    Ok(r.to_owned())
}

impl Db {
    /// Forfeits late proofs everywhere, then asks for proofs where a board's
    /// top has changed. Returns (player, pack) for each newly asked.
    pub async fn proofs_refresh(
        &self,
        pack: Option<String>,
        top_n: usize,
    ) -> rusqlite::Result<Vec<(i64, String)>> {
        self.run(move |c| {
            let tx = c.transaction()?;
            sweep(&tx)?;
            let packs: Vec<String> = match pack {
                Some(p) => vec![p],
                None => {
                    let mut st = tx.prepare(
                        "SELECT DISTINCT pack_id FROM ranked_attempts WHERE status = 'solved'",
                    )?;
                    let rows = st.query_map([], |r| r.get(0))?;
                    rows.collect::<rusqlite::Result<_>>()?
                }
            };
            let mut asked = Vec::new();
            for p in packs {
                for u in ensure(&tx, &p, top_n)? {
                    asked.push((u, p.clone()));
                }
            }
            tx.commit()?;
            Ok(asked)
        })
        .await
    }

    pub async fn proof_of(&self, user_id: i64, pack_id: String) -> rusqlite::Result<Option<Proof>> {
        self.run(move |c| {
            sweep(c)?;
            c.query_row(
                &format!("SELECT {PROOF_COLS} FROM proofs WHERE user_id = ?1 AND pack_id = ?2"),
                params![user_id, pack_id],
                row_proof,
            )
            .optional()
        })
        .await
    }

    /// The player's proofs still to write: asked for and not yet sent.
    pub async fn proofs_due(&self, user_id: i64) -> rusqlite::Result<Vec<Proof>> {
        self.run(move |c| {
            sweep(c)?;
            let mut st = c.prepare(&format!(
                "SELECT {PROOF_COLS} FROM proofs WHERE user_id = ?1 AND status = 'open' ORDER BY offered_at"
            ))?;
            let rows = st.query_map([user_id], row_proof)?;
            rows.collect()
        })
        .await
    }

    /// Starts the clock on an asked-for proof. False if it is not open, or
    /// already started.
    pub async fn proof_start(&self, user_id: i64, pack_id: String) -> rusqlite::Result<bool> {
        self.run(move |c| {
            sweep(c)?;
            let t = now();
            Ok(c.execute(
                "UPDATE proofs SET opened_at = ?3, deadline = ?3 + ?4
                 WHERE user_id = ?1 AND pack_id = ?2 AND status = 'open' AND opened_at IS NULL",
                params![user_id, pack_id, t, PROOF_SECONDS],
            )? == 1)
        })
        .await
    }

    /// Sends in a started proof, if it is before the deadline and grace.
    pub async fn proof_submit(
        &self,
        user_id: i64,
        pack_id: String,
        text: String,
    ) -> rusqlite::Result<bool> {
        self.run(move |c| {
            let t = now();
            let n = c.execute(
                "UPDATE proofs SET status = 'pending', text = ?3, submitted_at = ?4
                 WHERE user_id = ?1 AND pack_id = ?2 AND status = 'open'
                   AND opened_at IS NOT NULL AND ?4 <= deadline + ?5",
                params![user_id, pack_id, text, t, PROOF_GRACE],
            )?;
            Ok(n == 1)
        })
        .await
    }

    /// Proofs waiting for review, oldest first.
    pub async fn proofs_pending(&self) -> rusqlite::Result<Vec<PendingProof>> {
        self.run(|c| {
            let mut st = c.prepare(
                "SELECT p.id, u.username, p.pack_id, COALESCE(a.seconds, 0), COALESCE(a.hints, 0),
                        p.text, p.submitted_at
                 FROM proofs p JOIN users u ON u.id = p.user_id
                 LEFT JOIN ranked_attempts a ON a.user_id = p.user_id AND a.pack_id = p.pack_id
                 WHERE p.status = 'pending' ORDER BY p.submitted_at",
            )?;
            let rows = st.query_map([], |r| {
                Ok(PendingProof {
                    id: r.get(0)?,
                    username: r.get(1)?,
                    pack_id: r.get(2)?,
                    seconds: r.get(3)?,
                    hints: r.get(4)?,
                    text: r.get(5)?,
                    submitted_at: r.get(6)?,
                })
            })?;
            rows.collect()
        })
        .await
    }

    /// Approves or rejects a pending proof, audited, and asks the next player
    /// for one if a rejection opened a place. None if no pending proof has
    /// that id.
    pub async fn proof_review(
        &self,
        id: i64,
        reviewer: User,
        approve: bool,
        reason: Option<String>,
        top_n: usize,
    ) -> rusqlite::Result<Option<(String, Vec<i64>)>> {
        self.run(move |c| {
            let tx = c.transaction()?;
            let found: Option<(i64, String)> = tx
                .query_row(
                    "SELECT user_id, pack_id FROM proofs WHERE id = ?1 AND status = 'pending'",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((user, pack)) = found else {
                return Ok(None);
            };
            tx.execute(
                "UPDATE proofs SET status = ?2, reviewer_id = ?3, reviewed_at = ?4, reason = ?5 WHERE id = ?1",
                params![
                    id,
                    if approve { "approved" } else { "rejected" },
                    reviewer.id,
                    now(),
                    reason
                ],
            )?;
            audit(
                &tx,
                Actor::User(&reviewer),
                if approve { "proof approved" } else { "proof rejected" },
                Some(user),
                Some(&pack),
                reason.as_deref(),
            )?;
            let asked = ensure(&tx, &pack, top_n)?;
            tx.commit()?;
            Ok(Some((pack, asked)))
        })
        .await
    }

    /// Takes an entry off a ranked board (`reason` Some) or puts it back,
    /// audited, then asks for proofs where the top changed. The attempt's
    /// status and time are untouched. None if there is no such attempt.
    pub async fn board_set_removed(
        &self,
        user_id: i64,
        pack_id: String,
        by: User,
        reason: Option<String>,
        top_n: usize,
    ) -> rusqlite::Result<Option<Vec<i64>>> {
        self.run(move |c| {
            let tx = c.transaction()?;
            let n = match &reason {
                Some(r) => tx.execute(
                    "UPDATE ranked_attempts SET hidden = 1, hidden_by = ?3, hidden_reason = ?4
                     WHERE user_id = ?1 AND pack_id = ?2",
                    params![user_id, pack_id, by.id, r],
                )?,
                None => tx.execute(
                    "UPDATE ranked_attempts SET hidden = 0, hidden_by = NULL, hidden_reason = NULL
                     WHERE user_id = ?1 AND pack_id = ?2",
                    params![user_id, pack_id],
                )?,
            };
            if n != 1 {
                return Ok(None);
            }
            audit(
                &tx,
                Actor::User(&by),
                if reason.is_some() {
                    "entry removed"
                } else {
                    "entry reinstated"
                },
                Some(user_id),
                Some(&pack_id),
                reason.as_deref(),
            )?;
            let asked = ensure(&tx, &pack_id, top_n)?;
            tx.commit()?;
            Ok(Some(asked))
        })
        .await
    }

    /// Makes a user a moderator or not, audited. None if there is no such
    /// user; the user otherwise.
    pub async fn set_moderator(
        &self,
        username: String,
        on: bool,
        by: User,
    ) -> rusqlite::Result<Option<i64>> {
        self.run(move |c| {
            let tx = c.transaction()?;
            let id: Option<i64> = tx
                .query_row(
                    "SELECT id FROM users WHERE username = ?1 AND is_admin = 0",
                    [&username],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(id) = id else {
                return Ok(None);
            };
            tx.execute(
                "UPDATE users SET moderator = ?2 WHERE id = ?1",
                params![id, on],
            )?;
            audit(
                &tx,
                Actor::User(&by),
                if on {
                    "moderator added"
                } else {
                    "moderator removed"
                },
                Some(id),
                None,
                None,
            )?;
            tx.commit()?;
            Ok(Some(id))
        })
        .await
    }

    pub async fn moderators(&self) -> rusqlite::Result<Vec<String>> {
        self.run(|c| {
            let mut st = c.prepare(
                "SELECT username FROM users WHERE moderator = 1 ORDER BY username COLLATE NOCASE",
            )?;
            let rows = st.query_map([], |r| r.get(0))?;
            rows.collect()
        })
        .await
    }

    /// The newest audit entries.
    pub async fn audit_recent(&self, limit: i64) -> rusqlite::Result<Vec<AuditEntry>> {
        self.run(move |c| {
            let mut st = c.prepare(
                "SELECT at, actor, action, target, pack_id, reason FROM audit_log
                 ORDER BY id DESC LIMIT ?1",
            )?;
            let rows = st.query_map([limit], |r| {
                Ok(AuditEntry {
                    at: r.get(0)?,
                    actor: r.get(1)?,
                    action: r.get(2)?,
                    target: r.get(3)?,
                    pack_id: r.get(4)?,
                    reason: r.get(5)?,
                })
            })?;
            rows.collect()
        })
        .await
    }
}

// ---------------------------------------------------------------- notifying

/// Emails each newly asked player (but `except`, who is on the page already)
/// a link to their proof. Best effort.
pub async fn notify_asked(state: &AppState, asked: &[(i64, String)], except: Option<i64>) {
    if !state.mailer.enabled() {
        return;
    }
    for (user, pack) in asked {
        if Some(*user) == except {
            continue;
        }
        let Ok(Some(to)) = state.db.email_of(*user).await else {
            continue;
        };
        let title = state
            .get_any(pack)
            .map(|p| p.pack.meta.title.clone())
            .unwrap_or_else(|| pack.clone());
        let mail = crate::mail::Mail {
            to,
            subject: format!("Explain your solve of {title}"),
            body: format!(
                "Your ranked solve of {title} is in the top {} of its board, so it needs a proof: \
                 your own explanation of how you found the flag, written within {PROOF_SECONDS} seconds \
                 of pressing Start. Start it within 24 hours, or the entry is removed:\n\n{}/proof/{pack}\n",
                state.top_n, state.base_url
            ),
        };
        if let Err(e) = state.mailer.send(mail).await {
            tracing::warn!("proof email failed: {e}");
        }
    }
}

/// Refreshes proofs (all boards, or one) and emails whoever is newly asked.
pub async fn refresh(state: &AppState, pack: Option<String>, except: Option<i64>) {
    match state.db.proofs_refresh(pack, state.top_n).await {
        Ok(asked) => notify_asked(state, &asked, except).await,
        Err(e) => tracing::error!("refreshing proofs failed: {e}"),
    }
}

// ---------------------------------------------------------------- extractors

/// A signed-in moderator or admin. Others get a 404; visitors the login page.
pub struct ModeratorUser(pub User);

impl FromRequestParts<Arc<AppState>> for ModeratorUser {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let MaybeUser(user) = MaybeUser::from_request_parts(parts, state)
            .await
            .unwrap_or(MaybeUser(None));
        match user {
            Some(u) if u.can_moderate() => Ok(ModeratorUser(u)),
            Some(_) => Err(StatusCode::NOT_FOUND.into_response()),
            None => Err(Redirect::to("/login").into_response()),
        }
    }
}

fn server_error(e: rusqlite::Error) -> Response {
    tracing::error!("moderation failed: {e}");
    StatusCode::INTERNAL_SERVER_ERROR.into_response()
}

fn when(t: i64) -> String {
    crate::sealed::utc_time(t)
}

// ---------------------------------------------------------------- the player's proof

/// `GET /proof/{pack}`: the player's proof: start it, write it, or its result.
pub async fn proof_page(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    Path(pack): Path<String>,
) -> Response {
    let Some(user) = viewer else {
        return Redirect::to("/login").into_response();
    };
    let proof = match state.db.proof_of(user.id, pack.clone()).await {
        Ok(Some(p)) => p,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return server_error(e),
    };
    let title = state
        .get_any(&pack)
        .map(|p| p.pack.meta.title.clone())
        .unwrap_or_else(|| pack.clone());
    let t = now();
    let body = html! {
        section.wrap.page-head {
            h1 { "Proof: " (title) }
        }
        section.wrap.section-tight {
            div.auth.proof {
                @match (proof.status.as_str(), proof.opened_at) {
                    ("open", None) => {
                        p { "Your ranked solve is in the top " (state.top_n) " of the board, so it needs a proof: your own account of how you found the flag." }
                        ul {
                            li { "You have " (PROOF_SECONDS) " seconds from pressing Start. The box locks when they run out." }
                            li { "Start by " (when(proof.offered_at + PROOF_OPEN_WINDOW)) ", or the entry is removed." }
                            li { "A moderator reads it. A rejected or missing proof takes the entry off the board." }
                        }
                        p.notice { "AI-generated explanations are not accepted." }
                        form method="post" action={"/proof/" (pack) "/start"} {
                            button.button type="submit" { "Start (" (PROOF_SECONDS) " seconds)" }
                        }
                    }
                    ("open", Some(_)) => {
                        @let left = proof.deadline.unwrap_or(t) - t;
                        p.notice { "AI-generated explanations are not accepted." }
                        form #proof-form method="post" action={"/proof/" (pack)} data-left=(left.max(0)) {
                            div.field {
                                label for="proof-text" { "How did you find the flag?" }
                                textarea #proof-text name="text" rows="8" required
                                    minlength=(PROOF_MIN_CHARS) maxlength=(PROOF_MAX_CHARS) autofocus {}
                            }
                            p.muted { span #proof-left { (left.max(0)) } " seconds left." }
                            button.button type="submit" { "Send" }
                        }
                    }
                    ("pending", _) => p { "Sent. A moderator will review it." },
                    ("approved", _) => p.form-ok { "Approved. Your entry is verified on the board." },
                    ("rejected", _) => {
                        p.form-error { "Rejected, so the entry is off the board." }
                        @if let Some(r) = &proof.reason { p { "Reason: " (r) } }
                    }
                    _ => p.form-error { "Not sent in time, so the entry is off the board." },
                }
                p.auth-links { a href={"/leaderboard/ranked?pack=" (pack)} { "Ranked board" } }
            }
        }
    };
    layout(
        &state,
        Some(&user),
        "Proof",
        "Explain a ranked solve.",
        Nav::Leaderboard,
        &format!("/proof/{pack}"),
        body,
    )
    .into_response()
}

/// `POST /proof/{pack}/start`: starts the clock.
pub async fn proof_start(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    Path(pack): Path<String>,
) -> Response {
    let Some(user) = viewer else {
        return Redirect::to("/login").into_response();
    };
    match state.db.proof_start(user.id, pack.clone()).await {
        Ok(true) => Redirect::to(&format!("/proof/{pack}")).into_response(),
        Ok(false) => match state.db.proof_of(user.id, pack.clone()).await {
            Ok(Some(_)) => Redirect::to(&format!("/proof/{pack}")).into_response(),
            Ok(None) => StatusCode::NOT_FOUND.into_response(),
            Err(e) => server_error(e),
        },
        Err(e) => server_error(e),
    }
}

#[derive(Deserialize)]
pub struct ProofForm {
    text: String,
}

/// `POST /proof/{pack}`: sends the proof, before the deadline.
pub async fn proof_submit(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    Path(pack): Path<String>,
    Form(f): Form<ProofForm>,
) -> Response {
    let Some(user) = viewer else {
        return Redirect::to("/login").into_response();
    };
    let text = f.text.trim().to_owned();
    let n = text.chars().count();
    if !(PROOF_MIN_CHARS..=PROOF_MAX_CHARS).contains(&n) {
        return (
            StatusCode::BAD_REQUEST,
            format!("A proof is {PROOF_MIN_CHARS} to {PROOF_MAX_CHARS} characters."),
        )
            .into_response();
    }
    match state.db.proof_submit(user.id, pack.clone(), text).await {
        Ok(true) => {
            tracing::info!("proof sent by user {} for {pack}", user.id);
            Redirect::to(&format!("/proof/{pack}")).into_response()
        }
        Ok(false) => match state.db.proof_of(user.id, pack.clone()).await {
            Ok(Some(_)) => (
                StatusCode::CONFLICT,
                "Too late, or not started: this proof cannot be sent now.",
            )
                .into_response(),
            Ok(None) => StatusCode::NOT_FOUND.into_response(),
            Err(e) => server_error(e),
        },
        Err(e) => server_error(e),
    }
}

/// The "you owe a proof" notice, for pages a player with one due will see.
pub async fn due_notice(state: &AppState, viewer: Option<&User>) -> Markup {
    let Some(user) = viewer else {
        return html! {};
    };
    let due = state.db.proofs_due(user.id).await.unwrap_or_default();
    html! {
        @for p in due.iter().filter(|p| p.opened_at.is_none()) {
            p.notice.proof-due {
                "Your ranked solve of "
                (state.get_any(&p.pack_id).map(|r| r.pack.meta.title.clone()).unwrap_or_else(|| p.pack_id.clone()))
                " needs a proof. "
                a href={"/proof/" (p.pack_id)} { "Write it" }
                " by " (when(p.offered_at + PROOF_OPEN_WINDOW)) "."
            }
        }
    }
}

// ---------------------------------------------------------------- moderators

/// `GET /mod/proofs`: proofs waiting for review.
pub async fn mod_proofs(
    State(state): State<Arc<AppState>>,
    ModeratorUser(m): ModeratorUser,
) -> Response {
    refresh(&state, None, None).await;
    let pending = match state.db.proofs_pending().await {
        Ok(p) => p,
        Err(e) => return server_error(e),
    };
    let body = html! {
        section.wrap.page-head {
            h1 { "Proofs to review" }
            p.page-lead { "Approve a proof that explains the solve in the player's own words. Reject one that does not, or reads as AI-generated: the entry leaves the board and the next player is asked. Every decision is in the audit log." }
        }
        section.wrap.section-tight {
            @if pending.is_empty() {
                p.muted { "Nothing to review." }
            }
            @for p in &pending {
                article.proof-card {
                    h2 {
                        (p.username) " · "
                        a href={"/leaderboard/ranked?pack=" (p.pack_id)} {
                            (state.get_any(&p.pack_id).map(|r| r.pack.meta.title.clone()).unwrap_or_else(|| p.pack_id.clone()))
                        }
                    }
                    p.muted { (clock(p.seconds as f64)) ", " (p.hints) @if p.hints == 1 { " hint" } @else { " hints" } " · sent " (when(p.submitted_at)) }
                    blockquote.proof-text { (p.text) }
                    div.proof-actions {
                        form method="post" action={"/mod/proofs/" (p.id) "/approve"} {
                            button.button.button-quiet type="submit" { "Approve" }
                        }
                        form.inline-form method="post" action={"/mod/proofs/" (p.id) "/reject"} {
                            label.visually-hidden for={"reject-" (p.id)} { "Reason" }
                            input id={"reject-" (p.id)} name="reason" type="text" required maxlength="200" placeholder="Reason";
                            button.button.button-danger type="submit" { "Reject" }
                        }
                    }
                }
            }
        }
    };
    layout(
        &state,
        Some(&m),
        "Proofs to review",
        "Moderation.",
        Nav::Leaderboard,
        "/mod/proofs",
        body,
    )
    .into_response()
}

#[derive(Deserialize)]
pub struct ReasonForm {
    #[serde(default)]
    reason: String,
}

/// `POST /mod/proofs/{id}/{approve|reject}`.
pub async fn mod_review(
    State(state): State<Arc<AppState>>,
    ModeratorUser(m): ModeratorUser,
    Path((id, action)): Path<(i64, String)>,
    Form(f): Form<ReasonForm>,
) -> Response {
    let (approve, reason) = match action.as_str() {
        "approve" => (true, None),
        "reject" => match check_reason(&f.reason) {
            Ok(r) => (false, Some(r)),
            Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
        },
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    match state
        .db
        .proof_review(id, m.clone(), approve, reason, state.top_n)
        .await
    {
        Ok(Some((pack, asked))) => {
            let asked: Vec<(i64, String)> = asked.into_iter().map(|u| (u, pack.clone())).collect();
            notify_asked(&state, &asked, None).await;
            Redirect::to("/mod/proofs").into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => server_error(e),
    }
}

/// `POST /mod/board/{pack}/{user}/{remove|reinstate}`: takes an entry off a
/// ranked board, with a reason, or puts it back.
pub async fn mod_board(
    State(state): State<Arc<AppState>>,
    ModeratorUser(m): ModeratorUser,
    Path((pack, user, action)): Path<(String, i64, String)>,
    Form(f): Form<ReasonForm>,
) -> Response {
    let reason = match action.as_str() {
        "remove" => match check_reason(&f.reason) {
            Ok(r) => Some(r),
            Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
        },
        "reinstate" => None,
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    match state
        .db
        .board_set_removed(user, pack.clone(), m.clone(), reason, state.top_n)
        .await
    {
        Ok(Some(asked)) => {
            let asked: Vec<(i64, String)> = asked.into_iter().map(|u| (u, pack.clone())).collect();
            notify_asked(&state, &asked, None).await;
            Redirect::to(&format!("/leaderboard/ranked?pack={pack}")).into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => server_error(e),
    }
}

// ---------------------------------------------------------------- admins

/// `GET /admin/moderators`.
pub async fn admin_moderators(
    State(state): State<Arc<AppState>>,
    AdminUser(admin): AdminUser,
) -> Response {
    moderators_page(&state, &admin, None, StatusCode::OK).await
}

async fn moderators_page(
    state: &AppState,
    admin: &User,
    flash: Option<(bool, String)>,
    status: StatusCode,
) -> Response {
    let mods = match state.db.moderators().await {
        Ok(m) => m,
        Err(e) => return server_error(e),
    };
    let body = html! {
        section.wrap.page-head {
            h1 { "Moderators" }
            p.page-lead { "Moderators review proofs and remove or reinstate ranked board entries. Nothing else: every other admin page is closed to them." }
        }
        section.wrap.section-tight {
            div.auth {
                @if let Some((ok, text)) = &flash {
                    p.(if *ok { "form-ok" } else { "form-error" }) role="status" { (text) }
                }
                @if mods.is_empty() {
                    p.muted { "None yet." }
                } @else {
                    ul.admin-list {
                        @for name in &mods {
                            li {
                                (name) " "
                                form.inline-form method="post" action="/admin/moderators" {
                                    input type="hidden" name="username" value=(name);
                                    input type="hidden" name="action" value="remove";
                                    button.link-button type="submit" { "Remove" }
                                }
                            }
                        }
                    }
                }
                h2 { "Add a moderator" }
                form method="post" action="/admin/moderators" {
                    div.field {
                        label for="mod-username" { "Username" }
                        input #mod-username name="username" type="text" required maxlength="24" autocomplete="off";
                    }
                    input type="hidden" name="action" value="add";
                    button.button.button-quiet type="submit" { "Add" }
                }
                p.auth-links { a href="/admin/audit" { "Audit log" } " · " a href="/admin" { "Admin" } }
            }
        }
    };
    (
        status,
        layout(
            state,
            Some(admin),
            "Moderators",
            "Moderators.",
            Nav::Admin,
            "/admin/moderators",
            body,
        ),
    )
        .into_response()
}

#[derive(Deserialize)]
pub struct ModeratorForm {
    username: String,
    action: String,
}

/// `POST /admin/moderators`: `action` is `add` or `remove`.
pub async fn admin_moderators_post(
    State(state): State<Arc<AppState>>,
    AdminUser(admin): AdminUser,
    Form(f): Form<ModeratorForm>,
) -> Response {
    let on = match f.action.as_str() {
        "add" => true,
        "remove" => false,
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };
    let name = f.username.trim().to_owned();
    match state
        .db
        .set_moderator(name.clone(), on, admin.clone())
        .await
    {
        Ok(Some(_)) => {
            let text = if on {
                format!("{name} is a moderator.")
            } else {
                format!("{name} is no longer a moderator.")
            };
            moderators_page(&state, &admin, Some((true, text)), StatusCode::OK).await
        }
        Ok(None) => {
            moderators_page(
                &state,
                &admin,
                Some((false, format!("No player is called {name}."))),
                StatusCode::NOT_FOUND,
            )
            .await
        }
        Err(e) => server_error(e),
    }
}

/// `GET /admin/audit`: the newest 200 entries.
pub async fn admin_audit(
    State(state): State<Arc<AppState>>,
    AdminUser(admin): AdminUser,
) -> Response {
    let entries = match state.db.audit_recent(200).await {
        Ok(e) => e,
        Err(e) => return server_error(e),
    };
    let body = html! {
        section.wrap.page-head {
            h1 { "Audit log" }
            p.page-lead { "Every proof review, forfeit, board removal and reinstatement, and moderator change, newest first. Entries cannot be edited or removed from the site." }
        }
        section.wrap.section-tight {
            @if entries.is_empty() {
                p.muted { "Nothing yet." }
            } @else {
                div.table-wrap {
                    table.table.board {
                        thead { tr { th { "When (UTC)" } th { "Who" } th { "What" } th { "Player" } th { "Puzzle" } th { "Reason" } } }
                        tbody {
                            @for e in &entries {
                                tr {
                                    td { (when(e.at)) }
                                    td { (e.actor) }
                                    td { (e.action) }
                                    td { (e.target.as_deref().unwrap_or("")) }
                                    td { (e.pack_id.as_deref().unwrap_or("")) }
                                    td.player { (e.reason.as_deref().unwrap_or("")) }
                                }
                            }
                        }
                    }
                }
            }
            p.auth-links { a href="/admin/moderators" { "Moderators" } " · " a href="/admin" { "Admin" } }
        }
    };
    layout(
        &state,
        Some(&admin),
        "Audit log",
        "The audit log.",
        Nav::Admin,
        "/admin/audit",
        body,
    )
    .into_response()
}
