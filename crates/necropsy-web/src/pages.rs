// SPDX-License-Identifier: AGPL-3.0-or-later
//! Page handlers and HTML. Pages are built with `maud`, which escapes every
//! interpolated value; the only `PreEscaped` content is HTML we rendered from
//! pack markdown (raw HTML stripped) or from syntect (escaped).

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use maud::{html, Markup, PreEscaped, DOCTYPE};

use crate::auth::MaybeUser;
use crate::db::User;
use crate::markdown::render_markdown;
use crate::state::{AppState, RenderedPack};
use necropsy_pack::Track;

pub const CSS: &str = include_str!("../static/app.css");
pub const JS: &str = include_str!("../static/app.js");
pub const THEME_JS: &str = include_str!("../static/theme.js");
const FAVICON: &str = include_str!("../static/favicon.svg");
const FORMAT_DOC: &str = include_str!("../../../docs/pack-format.md");

const FONTS: &str = "https://fonts.googleapis.com/css2?family=Schibsted+Grotesk:ital,wght@0,400..800;1,400..800&family=JetBrains+Mono:wght@400;500&display=swap";

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Nav {
    Today,
    Learn,
    Library,
    Archive,
    Format,
    Account,
    Admin,
    None,
}

// ---------------------------------------------------------------- handlers

pub async fn today(State(state): State<Arc<AppState>>, MaybeUser(viewer): MaybeUser) -> Response {
    match state.today_daily() {
        Some(p) => puzzle_page(&state, viewer.as_ref(), p, &*state.stats().await).into_response(),
        // No daily has been released yet: start at the beginning.
        None => Redirect::to("/learn").into_response(),
    }
}

pub async fn puzzle(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    Path(id): Path<String>,
) -> Response {
    let v = viewer.as_ref();
    match state.get(&id) {
        Some(p) => puzzle_page(&state, v, p, &*state.stats().await).into_response(),
        None => not_found_page(&state, v).into_response(),
    }
}

/// `m:ss`, or `h:mm:ss` from an hour up.
fn clock(secs: f64) -> String {
    let s = secs.round().max(0.0) as i64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

type Stats = std::collections::HashMap<String, crate::db::PackStats>;

/// How many solved it, and how long it took on average, for a list row.
fn stats_text(st: Option<&crate::db::PackStats>) -> Option<String> {
    let st = st.filter(|s| s.solves > 0)?;
    Some(match st.avg_seconds.filter(|_| st.timed > 0) {
        Some(avg) => format!("{} solved · {} average", st.solves, clock(avg)),
        None => format!("{} solved", st.solves),
    })
}

fn pack_row(state: &AppState, p: &RenderedPack, stats: &Stats) -> Markup {
    let m = &p.pack.meta;
    let solved_by = stats_text(stats.get(&m.id));
    html! {
        li.work-row {
            a.work-link.archive-link href={"/p/" (m.id)} data-pack-id=(m.id) {
                span.work-name {
                    span.num { "#" (m.number) } " " (m.title)
                }
                span.work-desc {
                    @if let Some(d) = &m.date { span.date { (d) } " · " }
                    (m.tags.join(", "))
                    @if let Some(t) = &solved_by { " · " (t) }
                }
                span.work-meta {
                    @if state.is_today(p) { span.badge { "Today" } " " }
                    span.solved-mark.hidden { "Solved" }
                    " " (difficulty(m.difficulty))
                }
            }
        }
    }
}

pub async fn learn(State(state): State<Arc<AppState>>, MaybeUser(viewer): MaybeUser) -> Markup {
    let stats = state.stats().await;
    let body = html! {
        section.wrap.page-head {
            h1 { "Learn" }
            p.page-lead {
                "Four short puzzles on the basics: reading a log, decoding, deobfuscating, "
                "following a chain."
            }
        }
        section.wrap.section-tight {
            ul.work-list {
                @for p in state.intros() { (pack_row(&state, p, &stats)) }
            }
            @if state.today_daily().is_some() {
                p.after-list { a href="/" { "Then: today's puzzle →" } }
            }
        }
    };
    layout(
        &state,
        viewer.as_ref(),
        "Learn",
        "Short introductory puzzles.",
        Nav::Learn,
        "/learn",
        body,
    )
}

/// The library: puzzles that are not part of the daily rotation. Always available.
pub async fn library_list(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
) -> Markup {
    let stats = state.stats().await;
    let body = html! {
        section.wrap.page-head {
            h1 { "Library" }
            p.page-lead { "Puzzles outside the daily rotation. Always available, in any order." }
        }
        section.wrap.section-tight {
            ul.work-list {
                @for p in state.library() { (pack_row(&state, p, &stats)) }
            }
            @if viewer.is_none() {
                p.after-list { a href="/login" { "Log in" } " or " a href="/register" { "register" } " to start one." }
            }
        }
    };
    layout(
        &state,
        viewer.as_ref(),
        "Library",
        "Puzzles that are not part of the daily rotation.",
        Nav::Library,
        "/library",
        body,
    )
}

pub async fn archive(State(state): State<Arc<AppState>>, MaybeUser(viewer): MaybeUser) -> Markup {
    let stats = state.stats().await;
    let mut dailies = state.released_dailies();
    dailies.reverse();
    let body = html! {
        section.wrap.page-head {
            h1 { "Archive" }
            p.page-lead { "Every daily puzzle so far, newest first. They stay playable." }
        }
        section.wrap.section-tight {
            ul.work-list {
                @for p in dailies { (pack_row(&state, p, &stats)) }
            }
        }
    };
    layout(
        &state,
        viewer.as_ref(),
        "Archive",
        "All released daily puzzles.",
        Nav::Archive,
        "/archive",
        body,
    )
}

pub async fn format(State(state): State<Arc<AppState>>, MaybeUser(viewer): MaybeUser) -> Markup {
    let body = html! {
        section.wrap.page-head {
            h1 { "Pack format" }
        }
        section.wrap.section-tight {
            div.prose.doc { (PreEscaped(render_markdown(FORMAT_DOC))) }
        }
    };
    layout(
        &state,
        viewer.as_ref(),
        "Pack format",
        "How necropsy puzzle packs are structured.",
        Nav::Format,
        "/format",
        body,
    )
}

/// The raw bytes of one pack file. Lookup is by exact match against the files
/// the pack loader found, never by joining a request path onto a directory.
pub async fn raw_file(
    State(state): State<Arc<AppState>>,
    Path((id, path)): Path<(String, String)>,
) -> Response {
    let Some(file) = state.get(&id).and_then(|p| p.pack.file(&path)) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match file.text() {
        Some(text) => (
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            text.to_owned(),
        )
            .into_response(),
        None => (
            [
                (header::CONTENT_TYPE, "application/octet-stream"),
                (header::CONTENT_DISPOSITION, "attachment"),
            ],
            file.data.clone(),
        )
            .into_response(),
    }
}

pub async fn not_found(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
) -> Response {
    not_found_page(&state, viewer.as_ref()).into_response()
}

fn not_found_page(state: &AppState, viewer: Option<&User>) -> (StatusCode, Markup) {
    let body = html! {
        section.wrap.page-head {
            h1 { "Not found" }
            p.page-lead { a href="/" { "Back to today's puzzle" } }
        }
    };
    (
        StatusCode::NOT_FOUND,
        layout(
            state,
            viewer,
            "Not found",
            "Page not found.",
            Nav::None,
            "/",
            body,
        ),
    )
}

pub async fn static_css() -> Response {
    asset("text/css; charset=utf-8", CSS)
}
pub async fn static_js() -> Response {
    asset("text/javascript; charset=utf-8", JS)
}
pub async fn static_theme_js() -> Response {
    asset("text/javascript; charset=utf-8", THEME_JS)
}
pub async fn favicon() -> Response {
    asset("image/svg+xml", FAVICON)
}

fn asset(content_type: &'static str, body: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        body,
    )
        .into_response()
}

// ---------------------------------------------------------------- puzzle

/// The controls of a live puzzle: spin up an instance, see its link, its
/// time left and the requests it has had. Driven by app.js.
fn live_panel(state: &AppState, viewer: Option<&User>, rp: &RenderedPack) -> Markup {
    let captcha_key = state.captcha.site_key();
    let ttl = rp.pack.live.as_ref().map(|s| s.ttl_minutes).unwrap_or(60);
    html! {
        section #live.live-panel data-sitekey=[captcha_key] {
            h2 { "Live instance" }
            p.muted { "Runs for " (ttl) " minutes." }
            @if viewer.is_none() {
                p.notice { a href="/login" { "Sign in" } " to start an instance." }
            } @else {
                div #live-idle {
                    div #live-captcha.live-captcha {}
                    button #live-start.button type="button" { "Spin up instance" }
                }
                div #live-active.hidden {
                    p.live-url {
                        a #live-url target="_blank" rel="noopener noreferrer" {}
                        " "
                        button #live-copy.link-button type="button" { "Copy" }
                    }
                    p.muted {
                        span #live-left {} " left · "
                        button #live-stop.link-button type="button" { "Stop it" }
                    }
                    details.live-log-box {
                        summary { "Requests it has received" }
                        pre #live-log.live-log {}
                    }
                }
                p #live-msg.answer-msg role="status" aria-live="polite" {}
            }
        }
    }
}

fn puzzle_page(
    state: &AppState,
    viewer: Option<&User>,
    rp: &RenderedPack,
    stats: &Stats,
) -> Markup {
    let m = &rp.pack.meta;
    let (prev, next) = state.neighbors(rp);
    let share_url = format!("{}/p/{}", state.base_url, m.id);
    let daily = m.track == Track::Daily;
    let intro_ids = state
        .intros()
        .iter()
        .map(|p| p.pack.meta.id.clone())
        .collect::<Vec<_>>()
        .join(",");
    let release_day = m.release_day().map(|d| d.to_string()).unwrap_or_default();

    let body = html! {
        div #puzzle data-id=(m.id) data-number=(m.number) data-title=(m.title)
            data-hints=(m.hints.len()) data-url=(share_url)
            data-label=(rp.label().to_lowercase()) data-day=(release_day) {
            section.wrap.page-head {
                p.crumb {
                    @if daily {
                        (rp.label())
                        @if let Some(d) = &m.date { " · " (d) }
                        @if state.is_today(rp) { " " span.badge { "Today" } }
                    } @else if m.track == Track::Library {
                        "Library " (m.number) " of " (state.library().len())
                    } @else {
                        "Intro " (m.number) " of " (state.intros().len())
                    }
                }
                h1 { (m.title) }
            }
            div.wrap.case-grid {
                div.puzzle-main {
                    @if daily {
                        // Shown by app.js when nothing in the intro track is solved yet.
                        p #intro-banner.notice.hidden data-intros=(intro_ids) {
                            "New here? " a href="/learn" { "Start with the intro puzzles" } "."
                        }
                    }
                    div.prose.brief { (PreEscaped(&rp.brief_html)) }

                    @if rp.pack.is_live() {
                        (live_panel(state, viewer, rp))
                    }

                    @for f in &rp.files {
                        figure.codefile {
                            figcaption.codefile-head {
                                span.codefile-name { (f.path) }
                                a href={"/p/" (m.id) "/files/" (f.path)} { "Raw" }
                            }
                            @if let Some((hl, lines)) = &f.inline {
                                div.codefile-body tabindex="0" role="region"
                                    aria-label={"Contents of " (f.path)} {
                                    pre.gutter aria-hidden="true" {
                                        @for n in 1..=*lines { (n) "\n" }
                                    }
                                    pre.code.hl { code { (PreEscaped(hl)) } }
                                }
                            } @else {
                                p.codefile-bin {
                                    "Binary or large file, " (f.size) " bytes. "
                                    a href={"/p/" (m.id) "/files/" (f.path)} { "Download" }
                                }
                            }
                        }
                    }

                    form #answer-form.answer-form {
                        label for="answer" {
                            @if rp.pack.is_live() { "Flag" } @else { "Answer" }
                        }
                        div.answer-row {
                            input #answer name="answer" type="text" required maxlength="512"
                                autocomplete="off" autocapitalize="off" spellcheck="false";
                            button.button type="submit" { "Submit" }
                        }
                        p #answer-msg.answer-msg role="status" aria-live="polite" {}
                        noscript { p.muted { "Checking answers needs JavaScript." } }
                    }

                    @if !m.hints.is_empty() {
                        section #hints.hints {
                            ol #hint-list.hint-list {}
                            button #hint-btn.link-button type="button" { "Show a hint" }
                        }
                    }

                    section #explainer.explainer.hidden {
                        h2 { "Explainer" }
                        div #explainer-body.prose {}
                    }

                    nav.pager aria-label="Puzzles" {
                        @if let Some(p) = &prev {
                            a href=(p.href) { "← " (p.label) }
                        } @else { span {} }
                        @if let Some(n) = &next {
                            a href=(n.href) { (n.label) " →" }
                        }
                    }
                }

                aside.case-side {
                    dl.facts {
                        div { dt { "Difficulty" } dd { (difficulty(m.difficulty)) } }
                        div { dt { "Files" } dd { (rp.files.len()) } }
                        @if !m.tags.is_empty() {
                            div { dt { "Tags" } dd { (m.tags.join(", ")) } }
                        }
                        div { dt { "Time" } dd #stat-time.tabular { "0:00" } }
                        @if !m.hints.is_empty() {
                            div { dt { "Hints used" } dd #stat-hints.tabular { "0" } }
                        }
                    }
                    @if let Some(st) = stats.get(&m.id).filter(|s| s.solves > 0) {
                        dl.facts.facts-stats {
                            div { dt { "Solved by" } dd.tabular { (st.solves) } }
                            @if let (Some(avg), true) = (st.avg_seconds, st.timed > 0) {
                                div { dt { "Average time" } dd.tabular { (clock(avg)) } }
                            }
                            @if let (Some(avg), false, true) = (st.avg_hints, m.hints.is_empty(), st.timed > 0) {
                                div { dt { "Average hints" } dd.tabular { (format!("{avg:.1}")) } }
                            }
                        }
                    }
                    div #share.share.hidden {
                        button #share-btn.button.button-quiet type="button" { "Copy result" }
                        span #share-msg.muted role="status" aria-live="polite" {}
                    }
                }
            }
        }
    };

    layout(
        state,
        viewer,
        &m.title,
        &format!("necropsy puzzle #{}: {}", m.number, m.title),
        Nav::None,
        &format!("/p/{}", m.id),
        body,
    )
}

fn difficulty(d: u8) -> Markup {
    html! {
        span.diff role="img" aria-label={"Difficulty " (d) " of 5"} {
            @for i in 1..=5u8 { span.dot.on[i <= d] {} }
        }
    }
}

// ---------------------------------------------------------------- layout

pub fn layout(
    state: &AppState,
    viewer: Option<&User>,
    title: &str,
    description: &str,
    nav: Nav,
    path: &str,
    body: Markup,
) -> Markup {
    let full_title = format!("{title} · necropsy");
    let canonical = format!("{}{}", state.base_url, path);
    let current = |n: Nav| if nav == n { Some("page") } else { None };

    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (full_title) }
                meta name="description" content=(description);
                link rel="canonical" href=(canonical);
                meta property="og:type" content="website";
                meta property="og:site_name" content="necropsy";
                meta property="og:title" content=(full_title);
                meta property="og:description" content=(description);
                meta property="og:url" content=(canonical);
                meta name="theme-color" media="(prefers-color-scheme: light)" content="#eef0eb";
                meta name="theme-color" media="(prefers-color-scheme: dark)" content="#141916";
                link rel="icon" type="image/svg+xml" href={"/favicon.svg"};
                link rel="preconnect" href="https://fonts.googleapis.com";
                link rel="preconnect" href="https://fonts.gstatic.com" crossorigin;
                link rel="stylesheet" href=(FONTS);
                script src={"/static/theme.js?v=" (state.theme_v)} {}
                link rel="stylesheet" href={"/static/app.css?v=" (state.css_v)};
            }
            body {
                a.skip-link href="#main" { "Skip to content" }
                header.site-header {
                    div.wrap.header-inner {
                        a.site-name href="/" {
                            svg width="28" height="28" viewBox="0 0 32 32" aria-hidden="true" {
                                rect width="32" height="32" rx="8" fill="var(--accent)" {}
                                rect x="7" y="8.5" width="14" height="3.5" rx="1.75" fill="var(--accent-ink)" {}
                                rect x="11" y="14.25" width="14" height="3.5" rx="1.75" fill="var(--accent-ink)" {}
                                rect x="8.5" y="20" width="10" height="3.5" rx="1.75" fill="var(--accent-ink)" {}
                                circle cx="22.5" cy="21.75" r="1.75" fill="var(--accent-ink)" {}
                            }
                            span { "necropsy" }
                        }
                        nav.site-nav aria-label="Main" {
                            a href="/" aria-current=[current(Nav::Today)] { "Today" }
                            a href="/learn" aria-current=[current(Nav::Learn)] { "Learn" }
                            @if !state.library().is_empty() {
                                a href="/library" aria-current=[current(Nav::Library)] { "Library" }
                            }
                            a href="/archive" aria-current=[current(Nav::Archive)] { "Archive" }
                            a href="/format" aria-current=[current(Nav::Format)] { "Format" }
                        }
                        div.header-tools {
                            @if let Some(u) = viewer {
                                a.user-link.user-name href="/account" title=(u.username) aria-current=[current(Nav::Account)] { (u.username) }
                                @if u.is_admin {
                                    a.user-link href="/admin" aria-current=[current(Nav::Admin)] { "Admin" }
                                }
                            } @else {
                                a.user-link href="/login" { "Log in" }
                            }
                            a #streak-chip.streak.hidden href="/archive" title="Days in a row with a solved puzzle" {}
                            button #theme-toggle.icon-btn type="button"
                                aria-label="Switch color theme" title="Switch color theme" {
                                svg.icon width="18" height="18" viewBox="0 0 24 24" fill="none"
                                    stroke="currentColor" stroke-width="2" aria-hidden="true" {
                                    circle cx="12" cy="12" r="8" {}
                                    path d="M12 4a8 8 0 0 1 0 16Z" fill="currentColor" stroke="none" {}
                                }
                            }
                        }
                    }
                }
                main #main { (body) }
                footer.site-footer {
                    div.wrap.footer-inner {
                        a href="https://urashyl.dev" { "urashyl.dev" }
                        a href="https://github.com/urashyl/necropsy" rel="noopener noreferrer" { "Source" }
                    }
                }
                script src={"/static/app.js?v=" (state.js_v)} defer {}
            }
        }
    }
}
