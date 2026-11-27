// SPDX-License-Identifier: AGPL-3.0-or-later
//! Solve cards: a page and an image that say how long a puzzle took and how many hints
//! it needed, for a player to send to friends. See `docs/web.md`.
//!
//! A card exists only for a solve whose time and hints the server kept itself, so a
//! card cannot show numbers the player made up. The player turns sharing on for a solve,
//! which gives it a random token; the card is at `/card/<token>` (a page) and
//! `/card/<token>.svg` (the image), with no login needed. It shows the puzzle, the
//! time, the hints and the date, the streak for a daily solved on its day, and a row
//! of squares for the hints, and never the player's name, email or the answer.
//! Turning sharing off removes the token.
//!
//! The same facts go in the share text the player sends (see [`Card::share_text`]).

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use maud::{html, PreEscaped};
use serde::Serialize;

use crate::auth::{random_string, MaybeUser};
use crate::pages::{clock, layout, Nav};
use crate::state::AppState;

const TOKEN_ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
const TOKEN_LEN: usize = 16;

/// What a card says.
pub struct Card {
    pub title: String,
    /// "Daily #4", "Library #2".
    pub label: String,
    pub difficulty: u8,
    pub seconds: i64,
    pub hints: i64,
    /// How many hints the puzzle has.
    pub hint_total: i64,
    /// Days in a row, ending on this solve's day, with a daily solved on its own
    /// day. `None` unless this solve is one of them.
    pub streak: Option<i64>,
    /// `2026-11-28`, in the solver's own calendar.
    pub date: String,
    /// The site's host, shown at the foot of the image.
    pub host: String,
    /// The share of other solvers this one was faster than, when there are enough
    /// solves to say.
    pub faster_than: Option<u8>,
}

/// Each timed solve of the puzzle other than this one counts; with fewer than this
/// many others, a percentage would mean nothing.
const MIN_OTHERS_FOR_RANK: i64 = 4;

fn esc(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '&' => "&amp;".to_owned(),
            '<' => "&lt;".to_owned(),
            '>' => "&gt;".to_owned(),
            '"' => "&quot;".to_owned(),
            '\'' => "&#39;".to_owned(),
            c => c.to_string(),
        })
        .collect()
}

fn hints_text(n: i64) -> String {
    match n {
        0 => "no hints".to_owned(),
        1 => "1 hint".to_owned(),
        n => format!("{n} hints"),
    }
}

/// Days in a row ending on `day` that are in `days`: a streak as of that day.
pub fn streak_ending(days: &std::collections::HashSet<i64>, day: i64) -> i64 {
    let mut n = 0;
    while days.contains(&(day - n)) {
        n += 1;
    }
    n
}

/// One square per hint the puzzle has: yellow for each one used, green for each
/// one not needed. A puzzle without hints is one green square.
pub fn hint_grid(used: i64, total: i64) -> String {
    let total = total.max(0);
    let used = used.clamp(0, total);
    if total == 0 {
        return "🟩".to_owned();
    }
    "🟨".repeat(used as usize) + &"🟩".repeat((total - used) as usize)
}

impl Card {
    /// "1 of 3 hints", or "no hints" for a puzzle without any.
    fn hints_of(&self) -> String {
        if self.hint_total == 0 {
            "no hints".to_owned()
        } else {
            format!("{} of {} hints", self.hints, self.hint_total)
        }
    }

    /// What the player sends: which puzzle, the squares, the time, the hints, the
    /// streak and the link. No answer and nothing about how.
    ///
    /// ```text
    /// necropsy Daily #4 · Answer the door
    /// 🟨🟩 3:12 · 1 of 2 hints · 5-day streak
    /// https://necropsy.urashyl.dev/card/abcd...
    /// ```
    pub fn share_text(&self, url: &str) -> String {
        let mut line = format!(
            "{} {} · {}",
            hint_grid(self.hints, self.hint_total),
            clock(self.seconds as f64),
            self.hints_of()
        );
        if let Some(n) = self.streak.filter(|n| *n > 1) {
            line.push_str(&format!(" · {n}-day streak"));
        }
        format!("necropsy {} · {}\n{line}\n{url}", self.label, self.title)
    }

    /// "Solved in 3:12 with 1 hint."
    pub fn summary(&self) -> String {
        let mut s = format!(
            "Solved in {} with {}.",
            clock(self.seconds as f64),
            hints_text(self.hints)
        );
        if let Some(n) = self.streak.filter(|n| *n > 1) {
            s.push_str(&format!(" {n}-day streak."));
        }
        if let Some(p) = self.faster_than {
            s.push_str(&format!(" Faster than {p}% of solvers."));
        }
        s
    }

    /// The card as an SVG image, 1200 by 630, the proportions link previews use.
    pub fn svg(&self) -> String {
        const INK: &str = "#eef0eb";
        const MUTED: &str = "#8fa196";
        const ACCENT: &str = "#2b5e4f";
        const BRIGHT: &str = "#7fd1b0";
        // A long title gets a smaller type size, then a cut, so it never leaves the card.
        let mut title: String = self.title.chars().take(44).collect();
        if self.title.chars().count() > 44 {
            title.push('…');
        }
        let size = (1072.0 / (title.chars().count().max(1) as f64 * 0.56)).clamp(44.0, 80.0);
        let dots: String = (0..5)
            .map(|i| {
                let filled = (i as u8) < self.difficulty;
                format!(
                    r##"<circle cx="{}" cy="486" r="15" fill="{}" stroke="{BRIGHT}" stroke-width="3"/>"##,
                    799 + i * 44,
                    if filled { BRIGHT } else { "none" }
                )
            })
            .collect();
        // The hint squares, under the hint count.
        let squares: String = if self.hint_total == 0 {
            String::new()
        } else {
            (0..self.hint_total.min(8))
                .map(|i| {
                    format!(
                        r##"<rect x="{}" y="520" width="28" height="28" rx="5" fill="{}"/>"##,
                        480 + i * 38,
                        if i < self.hints { "#e0b35a" } else { BRIGHT }
                    )
                })
                .collect()
        };
        let streak = self
            .streak
            .filter(|n| *n > 1)
            .map_or_else(String::new, |n| format!(" · {n}-day streak"));
        let hints_big = if self.hint_total == 0 {
            "0".to_owned()
        } else {
            format!("{}/{}", self.hints, self.hint_total)
        };
        let rank = self.faster_than.map_or_else(String::new, |p| {
            format!(
                r##"<text x="1136" y="590" text-anchor="end" font-family="Schibsted Grotesk, system-ui, sans-serif" font-size="26" fill="{BRIGHT}">Faster than {p}% of solvers</text>"##
            )
        });
        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1200 630" width="1200" height="630" role="img" aria-label="{aria}">
<rect width="1200" height="630" fill="#141916"/>
<rect x="4" y="4" width="1192" height="622" rx="24" fill="none" stroke="{ACCENT}" stroke-width="8"/>
<g transform="translate(64 56) scale(1.75)">
<rect width="32" height="32" rx="8" fill="{ACCENT}"/>
<rect x="7" y="8.5" width="14" height="3.5" rx="1.75" fill="#f4f7f3"/>
<rect x="11" y="14.25" width="14" height="3.5" rx="1.75" fill="#f4f7f3"/>
<rect x="8.5" y="20" width="10" height="3.5" rx="1.75" fill="#f4f7f3"/>
<circle cx="22.5" cy="21.75" r="1.75" fill="#f4f7f3"/>
</g>
<text x="140" y="96" font-family="Schibsted Grotesk, system-ui, sans-serif" font-size="38" font-weight="650" fill="{INK}">necropsy</text>
<text x="1136" y="96" text-anchor="end" font-family="Schibsted Grotesk, system-ui, sans-serif" font-size="30" fill="{MUTED}">{label}</text>
<text x="64" y="250" font-family="Schibsted Grotesk, system-ui, sans-serif" font-size="{size:.0}" font-weight="700" fill="{INK}">{title}</text>
<g font-family="Schibsted Grotesk, system-ui, sans-serif" font-size="22" letter-spacing="3" fill="{MUTED}">
<text x="64" y="378">TIME</text>
<text x="480" y="378">HINTS</text>
<text x="784" y="378">DIFFICULTY</text>
</g>
<g font-family="JetBrains Mono, ui-monospace, Menlo, monospace" font-size="104" font-weight="700" fill="{BRIGHT}">
<text x="64" y="500">{time}</text>
<text x="480" y="500">{hints}</text>
</g>
{squares}
{dots}
<text x="64" y="590" font-family="Schibsted Grotesk, system-ui, sans-serif" font-size="26" fill="{MUTED}">{host} · solved {date}{streak}</text>
{rank}
</svg>"##,
            aria = esc(&format!("{title}. {}", self.summary())),
            label = esc(&self.label),
            title = esc(&title),
            time = esc(&clock(self.seconds as f64)),
            hints = esc(&hints_big),
            host = esc(&self.host),
            date = esc(&self.date),
        )
    }
}

#[derive(Serialize)]
struct ShareRes {
    url: String,
    /// The text to send with it, see [`Card::share_text`].
    text: String,
}

/// Turns sharing on for the signed-in player's solve of a puzzle, and returns the card's
/// address. The same address comes back each time.
pub async fn share_post(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    Path(id): Path<String>,
) -> Response {
    let Some(user) = viewer else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if !state.limiter.allow(
        &format!("share:{}", user.id),
        30,
        std::time::Duration::from_secs(60),
    ) {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let candidate = random_string(TOKEN_ALPHABET, TOKEN_LEN);
    match state.db.share_enable(user.id, id, candidate).await {
        Ok(Some(token)) => {
            let url = format!("{}/card/{token}", state.base_url);
            match card_for(&state, &token).await {
                Some((_, card)) => Json(ShareRes {
                    text: card.share_text(&url),
                    url,
                })
                .into_response(),
                None => StatusCode::NOT_FOUND.into_response(),
            }
        }
        // No solve, or one the server did not time: there is nothing a card could vouch for.
        Ok(None) => (
            StatusCode::CONFLICT,
            "This solve has no recorded time, so it has no card.",
        )
            .into_response(),
        Err(e) => {
            tracing::error!("sharing a solve failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Turns sharing off: the card's page and image stop working.
pub async fn share_delete(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    Path(id): Path<String>,
) -> Response {
    let Some(user) = viewer else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    match state.db.share_disable(user.id, id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            tracing::error!("unsharing a solve failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

fn valid_token(t: &str) -> bool {
    t.len() == TOKEN_LEN && t.bytes().all(|b| TOKEN_ALPHABET.contains(&b))
}

async fn card_for(state: &AppState, token: &str) -> Option<(String, Card)> {
    if !valid_token(token) {
        return None;
    }
    let s = state
        .db
        .shared_solve(token.to_owned())
        .await
        .ok()
        .flatten()?;
    // Any loaded puzzle: the player solved it, so it was open to them.
    let rp = state.get_any(&s.pack_id)?;
    let (timed, slower) = state
        .db
        .slower_than(s.pack_id.clone(), s.seconds)
        .await
        .unwrap_or((0, 0));
    let others = timed - 1;
    let faster_than =
        (others >= MIN_OTHERS_FOR_RANK).then(|| (slower * 100 / others).clamp(0, 100) as u8);
    // The streak as of the solve's own day: dailies solved on their release day,
    // in the solver's calendar at the time.
    let day_of = |at: i64, tz: i64| (at + tz * 60).div_euclid(86_400);
    let solve_day = day_of(s.solved_at, s.tz);
    let streak = if rp.pack.meta.release_day() == Some(solve_day) {
        let days: std::collections::HashSet<i64> = state
            .db
            .solves_of(s.user_id)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter_map(|x| {
                let d = day_of(x.solved_at, x.tz);
                state
                    .get_any(&x.pack_id)
                    .and_then(|p| p.pack.meta.release_day())
                    .filter(|r| *r == d)
            })
            .collect();
        Some(streak_ending(&days, solve_day))
    } else {
        None
    };
    let host = state
        .base_url
        .split_once("://")
        .map_or(state.base_url.as_str(), |(_, h)| h)
        .to_owned();
    Some((
        rp.pack.meta.id.clone(),
        Card {
            title: rp.pack.meta.title.clone(),
            label: rp.label(),
            difficulty: rp.pack.meta.difficulty,
            seconds: s.seconds,
            hints: s.hints,
            hint_total: rp.pack.meta.hints.len() as i64,
            streak,
            date: crate::accounts::format_date(s.solved_at + s.tz * 60),
            host,
            faster_than,
        },
    ))
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        [(header::CACHE_CONTROL, "no-store")],
        "Not found.\n",
    )
        .into_response()
}

/// `/card/<token>` is the page, `/card/<token>.svg` the image.
pub async fn card_get(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    Path(file): Path<String>,
) -> Response {
    let (token, image) = match file.strip_suffix(".svg") {
        Some(t) => (t, true),
        None => (file.as_str(), false),
    };
    let Some((pack_id, card)) = card_for(&state, token).await else {
        return not_found();
    };
    // Never indexed: a card is for the people it was sent to.
    let noindex = (
        header::HeaderName::from_static("x-robots-tag"),
        "noindex, nofollow",
    );
    if image {
        return (
            [
                (header::CONTENT_TYPE, "image/svg+xml"),
                (header::CACHE_CONTROL, "public, max-age=300"),
            ],
            [noindex],
            card.svg(),
        )
            .into_response();
    }
    let body = html! {
        section.wrap.page-head {
            h1 { (card.title) }
            p.page-lead { (card.summary()) }
        }
        section.wrap.section-tight {
            div.card-view { (PreEscaped(card.svg())) }
            p.card-actions {
                a.button href={"/p/" (pack_id)} { "Play it" }
                " "
                a.button.button-quiet href="/learn" { "New to this? Learn" }
                " "
                button.button.button-quiet #card-save type="button" data-src={"/card/" (token) ".svg"} data-name=(pack_id) {
                    "Save image"
                }
                span #card-msg.muted role="status" aria-live="polite" {}
            }
        }
    };
    (
        [noindex],
        layout(
            &state,
            viewer.as_ref(),
            &format!("{} solved in {}", card.title, clock(card.seconds as f64)),
            &card.summary(),
            Nav::None,
            &format!("/card/{token}"),
            body,
        ),
    )
        .into_response()
}

#[cfg(test)]
mod unit {
    use super::*;

    fn card() -> Card {
        Card {
            title: "Who got in?".into(),
            label: "Intro #1".into(),
            difficulty: 2,
            seconds: 192,
            hints: 1,
            hint_total: 3,
            streak: Some(5),
            date: "2026-11-28".into(),
            host: "necropsy.example".into(),
            faster_than: Some(72),
        }
    }

    #[test]
    fn the_summary_and_image_carry_the_numbers() {
        let c = card();
        assert_eq!(
            c.summary(),
            "Solved in 3:12 with 1 hint. 5-day streak. Faster than 72% of solvers."
        );
        let svg = c.svg();
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains(">3:12<") && svg.contains("Who got in?"));
        assert!(svg.contains("Faster than 72% of solvers"));
        assert!(svg.contains("necropsy.example · solved 2026-11-28"));
        assert!(svg.contains(">1/3<") && svg.contains("5-day streak"));
        assert_eq!(svg.matches("rx=\"5\"").count(), 3, "one square per hint");
        let none = Card {
            hints: 0,
            faster_than: None,
            streak: None,
            ..card()
        };
        assert_eq!(none.summary(), "Solved in 3:12 with no hints.");
        assert!(!none.svg().contains("Faster than"));
        let many = Card { hints: 3, ..card() };
        assert!(many.summary().contains("3 hints"));
    }

    #[test]
    fn text_is_escaped_and_a_long_title_is_cut() {
        let c = Card {
            title: "<script>alert(1)</script> & \"quotes\"".into(),
            label: "x\" onload=\"y".into(),
            ..card()
        };
        let svg = c.svg();
        assert!(!svg.contains("<script>"), "{svg}");
        assert!(!svg.contains("onload=\"y"));
        assert!(svg.contains("&lt;script&gt;"));
        let long = Card {
            title: "a".repeat(200),
            ..card()
        };
        let svg = long.svg();
        assert!(svg.contains('…') && !svg.contains(&"a".repeat(60)));
    }

    #[test]
    fn the_share_text_has_the_numbers_and_no_answer() {
        let c = card();
        assert_eq!(
            c.share_text("https://x.test/card/t"),
            "necropsy Intro #1 · Who got in?\n🟨🟩🟩 3:12 · 1 of 3 hints · 5-day streak\nhttps://x.test/card/t"
        );
        // A streak of one day is no streak; a puzzle without hints has one square.
        let c = Card {
            streak: Some(1),
            hints: 0,
            hint_total: 0,
            ..card()
        };
        assert_eq!(
            c.share_text("u"),
            "necropsy Intro #1 · Who got in?\n🟩 3:12 · no hints\nu"
        );
    }

    #[test]
    fn grids_and_streaks() {
        assert_eq!(hint_grid(0, 2), "🟩🟩");
        assert_eq!(hint_grid(2, 2), "🟨🟨");
        assert_eq!(hint_grid(5, 2), "🟨🟨", "never more than the puzzle has");
        assert_eq!(hint_grid(0, 0), "🟩");
        let days: std::collections::HashSet<i64> = [10, 11, 12, 14].into_iter().collect();
        assert_eq!(streak_ending(&days, 12), 3);
        assert_eq!(streak_ending(&days, 14), 1);
        assert_eq!(streak_ending(&days, 13), 0);
    }

    #[test]
    fn tokens_have_one_shape() {
        assert!(valid_token(&random_string(TOKEN_ALPHABET, TOKEN_LEN)));
        for bad in [
            "",
            "short",
            "ABCDEFGHJKMNPQRS",
            "abcdefghjkmnpqr!",
            "abcdefghjkmnpqrst",
        ] {
            assert!(!valid_token(bad), "{bad}");
        }
    }
}
