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
    Search,
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
pub(crate) fn clock(secs: f64) -> String {
    let s = secs.round().max(0.0) as i64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

pub(crate) type Stats = std::collections::HashMap<String, crate::db::PackStats>;

/// How many solved it, and how long it took on average, for a list row.
fn stats_text(st: Option<&crate::db::PackStats>) -> Option<String> {
    let st = st.filter(|s| s.solves > 0)?;
    Some(match st.avg_seconds.filter(|_| st.timed > 0) {
        Some(avg) => format!("{} solved · {} average", st.solves, clock(avg)),
        None => format!("{} solved", st.solves),
    })
}

pub(crate) fn pack_row(state: &AppState, p: &RenderedPack, stats: &Stats) -> Markup {
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

/// `%XX`-encodes everything but unreserved characters, for a query value.
pub(crate) fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn search_form(q: &str) -> Markup {
    html! {
        form.search-form method="get" action="/search" role="search" {
            input name="q" type="search" value=(q) placeholder="Search puzzles" aria-label="Search puzzles"
                maxlength="100" autocomplete="off";
            button.button type="submit" { "Search" }
        }
    }
}

#[derive(serde::Deserialize, Default)]
pub struct SearchQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    tag: String,
}

/// Puzzles whose title, tags, id or brief contain every word of the query, best match
/// first. Only puzzles the player can open are searched, and never explainers or hints,
/// which hold the answer.
fn search_matches<'a>(state: &'a AppState, q: &str, tag: &str) -> Vec<&'a RenderedPack> {
    let words: Vec<String> = q.split_whitespace().map(str::to_lowercase).collect();
    let tag = tag.trim().to_lowercase();
    let mut hits: Vec<(u8, &RenderedPack)> = Vec::new();
    for p in state.visible_packs() {
        let m = &p.pack.meta;
        if !tag.is_empty() && !m.tags.iter().any(|t| t.to_lowercase() == tag) {
            continue;
        }
        let title = m.title.to_lowercase();
        let tags = m.tags.join(" ").to_lowercase();
        let id = m.id.to_lowercase();
        let brief = p.pack.brief_md.to_lowercase();
        let mut score = 0u8;
        let mut all = true;
        for w in &words {
            let s = if title.contains(w) {
                4
            } else if tags.contains(w) || id.contains(w) {
                3
            } else if brief.contains(w) {
                1
            } else {
                0
            };
            if s == 0 {
                all = false;
                break;
            }
            score = score.saturating_add(s);
        }
        if all {
            hits.push((score, p));
        }
    }
    // Best first; the puzzle order breaks ties.
    hits.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    hits.into_iter().map(|(_, p)| p).take(100).collect()
}

pub async fn search(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    axum::extract::Query(sq): axum::extract::Query<SearchQuery>,
) -> Markup {
    let stats = state.stats().await;
    let q: String = sq.q.chars().take(100).collect();
    let tag: String = sq.tag.chars().take(50).collect();
    let searching = !q.trim().is_empty() || !tag.trim().is_empty();
    let found = if searching {
        search_matches(&state, &q, &tag)
    } else {
        Vec::new()
    };
    // Every tag on a puzzle the player can open, with how many carry it.
    let mut tags: std::collections::BTreeMap<String, (String, usize)> = Default::default();
    if !searching {
        for p in state.visible_packs() {
            for t in &p.pack.meta.tags {
                tags.entry(t.to_lowercase())
                    .or_insert_with(|| (t.clone(), 0))
                    .1 += 1;
            }
        }
    }
    let body = html! {
        section.wrap.page-head {
            h1 { "Search" }
            (search_form(&q))
        }
        section.wrap.section-tight {
            @if searching {
                p.muted {
                    @if !tag.trim().is_empty() { "Tagged " strong { (tag.trim()) } ". " }
                    (found.len()) @if found.len() == 1 { " puzzle." } @else { " puzzles." }
                    " " a href="/search" { "All tags" }
                }
                ul.work-list {
                    @for p in &found { (pack_row(&state, p, &stats)) }
                }
            } @else {
                ul.tag-list {
                    @for (_, (name, n)) in &tags {
                        li { a.tag href={"/search?tag=" (urlencode(name))} { (name) " " span.muted { (n) } } }
                    }
                }
            }
        }
    };
    layout(
        &state,
        viewer.as_ref(),
        "Search",
        "Search puzzles by title, tag or brief.",
        Nav::Search,
        "/search",
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

pub(crate) fn not_found_page(state: &AppState, viewer: Option<&User>) -> (StatusCode, Markup) {
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
pub(crate) fn live_panel(state: &AppState, viewer: Option<&User>, rp: &RenderedPack) -> Markup {
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
    let daily = m.track == Track::Daily;
    let intro_ids = state
        .intros()
        .iter()
        .map(|p| p.pack.meta.id.clone())
        .collect::<Vec<_>>()
        .join(",");

    let body = html! {
        div #puzzle data-id=(m.id) data-number=(m.number) data-title=(m.title)
            data-hints=(m.hints.len()) data-url=(puzzle_url(state, rp))
            data-label=(rp.label().to_lowercase()) data-day=(release_day(rp)) {
            section.wrap.page-head {
                (crumb(state, rp))
                h1 { (m.title) }
            }
            div.wrap.case-grid {
                div.puzzle-main {
                    @if daily {
                        // Shown by app.js when nothing in the intro track is solved yet.
                        p #intro-banner.notice.hidden data-intros=(intro_ids) {
                            "New here? " a href="/learn" { "Start with Learn" } "."
                        }
                    }
                    div.prose.brief { (PreEscaped(&rp.brief_html)) }

                    p.workspace-link {
                        a.button.button-quiet href={"/w/" (m.id)} { "Open the workspace" }
                        " "
                        span.muted { "Pcap viewer, decoder, "
                            @if rp.pack.is_live() { "request console, " }
                            "notebook and hex viewer, in the browser." }
                    }

                    @if rp.pack.is_live() {
                        (live_panel(state, viewer, rp))
                    }

                    (file_figures(rp))

                    (answer_block(rp))

                    @if let Some(link) = crate::community::discussion_link(state, rp) {
                        p.discuss {
                            a href=(link) rel="noopener noreferrer" {
                                @if state.community.thread(&m.id).is_some() {
                                    "Discuss it on Discord"
                                } @else {
                                    "Discuss closed dailies on Discord"
                                }
                            }
                        }
                    } @else if daily && state.community.on() {
                        // Shown by app.js once solved: there is no thread for an open daily.
                        p #discuss-later.muted.hidden {
                            "The discussion opens once this daily has closed, when its date is over in every timezone."
                        }
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
                    (side_facts(rp, stats))
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

/// The puzzle's public address, for the share text.
pub(crate) fn puzzle_url(state: &AppState, rp: &RenderedPack) -> String {
    format!("{}/p/{}", state.base_url, rp.pack.meta.id)
}

/// A daily's release day as days since 1970, or "" for other tracks.
pub(crate) fn release_day(rp: &RenderedPack) -> String {
    rp.pack
        .meta
        .release_day()
        .map(|d| d.to_string())
        .unwrap_or_default()
}

/// "Daily #3 · 2026-11-27 Today", "Library 2 of 10" or "Intro 1 of 4".
pub(crate) fn crumb(state: &AppState, rp: &RenderedPack) -> Markup {
    let m = &rp.pack.meta;
    html! {
        p.crumb {
            @if m.track == Track::Daily {
                (rp.label())
                @if let Some(d) = &m.date { " · " (d) }
                @if state.is_today(rp) { " " span.badge { "Today" } }
            } @else if m.track == Track::Library {
                "Library " (position(&state.library(), rp)) " of " (state.library().len())
            } @else {
                "Intro " (position(&state.intros(), rp)) " of " (state.intros().len())
            }
        }
    }
}

/// Where a puzzle is in its list, counting from 1. Hidden puzzles (live ones on
/// a site without live puzzles) are not in the list, so they leave no gap.
fn position(list: &[&RenderedPack], rp: &RenderedPack) -> usize {
    list.iter()
        .position(|p| p.pack.meta.id == rp.pack.meta.id)
        .map_or(0, |i| i + 1)
}

/// Each file of the puzzle: text inline with line numbers, anything else as a
/// download.
pub(crate) fn file_figures(rp: &RenderedPack) -> Markup {
    let m = &rp.pack.meta;
    html! {
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
    }
}

/// The answer form, the hints and the explainer, driven by app.js.
pub(crate) fn answer_block(rp: &RenderedPack) -> Markup {
    let m = &rp.pack.meta;
    html! {
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
    }
}

/// Difficulty, files, tags, the clock, the solve statistics and the share buttons.
pub(crate) fn side_facts(rp: &RenderedPack, stats: &Stats) -> Markup {
    let m = &rp.pack.meta;
    html! {
        dl.facts {
            div { dt { "Difficulty" } dd { (difficulty(m.difficulty)) } }
            div { dt { "Files" } dd { (rp.files.len()) } }
            @if !m.tags.is_empty() {
                div { dt { "Tags" } dd.tag-links {
                    @for (i, t) in m.tags.iter().enumerate() {
                        @if i > 0 { ", " }
                        a href={"/search?tag=" (urlencode(t))} { (t) }
                    }
                } }
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
        div #share.share.hidden data-label=(rp.label()) {
            pre #share-preview.share-preview {}
            button #share-btn.button type="button" { "Share" }
            button #share-save.button.button-quiet.hidden type="button" { "Save image" }
            button #share-stop.link-button.hidden type="button" { "Stop sharing" }
            span #share-msg.muted role="status" aria-live="polite" {}
        }
    }
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
                        a.site-name href="/" aria-label="necropsy" {
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
                            a.icon-btn href="/search" aria-label="Search" title="Search"
                                aria-current=[current(Nav::Search)] {
                                svg.icon width="18" height="18" viewBox="0 0 24 24" fill="none"
                                    stroke="currentColor" stroke-width="2" stroke-linecap="round"
                                    aria-hidden="true" {
                                    circle cx="11" cy="11" r="7" {}
                                    path d="m20 20-3.5-3.5" {}
                                }
                            }
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
                        @if let Some(d) = &state.community.invite {
                            a href=(d) rel="noopener noreferrer" { "Discord" }
                        }
                    }
                }
                script src={"/static/app.js?v=" (state.js_v)} defer {}
            }
        }
    }
}
