// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Learn track: modules from a first capture to a 5/5 daily, each with a
//! concept, a walkthrough for the browser path and one for the terminal path,
//! practice puzzles and a checkpoint. See `docs/learn.md`.
//!
//! The path is a preference, not progress: switching it changes which
//! walkthrough is shown and nothing else. Checkpoints accept the answers of
//! either path. A signed-in player's passed checkpoints are stored on the
//! server; a signed-out player's are kept by app.js in the browser and sent
//! to the server after signing in, like solves.

use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Form, Json};
use maud::{html, Markup, PreEscaped};
use necropsy_pack::learn::{Kind, Module};
use serde::{Deserialize, Serialize};

use crate::auth::MaybeUser;
use crate::db::User;
use crate::markdown::render_markdown;
use crate::pages::{layout, not_found_page, pack_row, Nav, Stats};
use crate::state::{AppState, RenderedPack};

/// The cookie that keeps a signed-out player's path.
const PATH_COOKIE: &str = "necropsy_path";

/// A module, with its markdown rendered once at startup.
pub struct LearnModule {
    pub module: Module,
    pub concept_html: String,
    pub browser_html: String,
    pub terminal_html: String,
}

impl LearnModule {
    pub fn new(module: Module) -> Self {
        Self {
            concept_html: render_markdown(&module.concept_md),
            browser_html: render_markdown(&module.browser_md),
            terminal_html: render_markdown(&module.terminal_md),
            module,
        }
    }

    fn id(&self) -> &str {
        &self.module.meta.id
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToolPath {
    Browser,
    Terminal,
}

impl ToolPath {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "browser" => Some(Self::Browser),
            "terminal" => Some(Self::Terminal),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Browser => "browser",
            Self::Terminal => "terminal",
        }
    }
}

/// The viewer's chosen path: their account's, else the cookie's. `None` when
/// they have not chosen.
async fn chosen_path(
    state: &AppState,
    viewer: Option<&User>,
    headers: &HeaderMap,
) -> Option<ToolPath> {
    if let Some(u) = viewer {
        if let Ok(Some(p)) = state.db.learn_path(u.id).await {
            return ToolPath::parse(&p);
        }
    }
    crate::auth::cookie_value(headers, PATH_COOKIE).and_then(|v| ToolPath::parse(&v))
}

async fn passed_of(state: &AppState, viewer: Option<&User>) -> HashSet<String> {
    match viewer {
        Some(u) => state
            .db
            .learn_passed(u.id)
            .await
            .unwrap_or_default()
            .into_iter()
            .collect(),
        None => HashSet::new(),
    }
}

fn tier_name(t: u8) -> &'static str {
    match t {
        1 => "Tier 1: first steps",
        2 => "Tier 2: talking to servers",
        3 => "Tier 3: crypto, binaries, scripts",
        4 => "Tier 4: your own tools",
        _ => "Tier 5: full intrusions",
    }
}

fn path_form(current: Option<ToolPath>, back: &str) -> Markup {
    html! {
        form.path-form method="post" action="/learn/path" {
            input type="hidden" name="back" value=(back);
            fieldset {
                legend { "Tools" }
                div.path-options {
                    button.path-option type="submit" name="path" value="browser"
                        aria-pressed=(current == Some(ToolPath::Browser)) {
                        strong { "In the browser" }
                        span { "Nothing to install. The workspace has a pcap viewer, decoder, request console and Python." }
                    }
                    button.path-option type="submit" name="path" value="terminal"
                        aria-pressed=(current == Some(ToolPath::Terminal)) {
                        strong { "In a terminal" }
                        span { "Wireshark, tshark, curl and Python on your own computer, from the first module." }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------- index

/// `GET /learn`.
pub async fn index(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    headers: HeaderMap,
) -> Markup {
    let stats = state.stats().await;
    let path = chosen_path(&state, viewer.as_ref(), &headers).await;
    let passed = passed_of(&state, viewer.as_ref()).await;
    let modules = &state.learn;
    let tiers: Vec<u8> = {
        let mut t: Vec<u8> = modules.iter().map(|m| m.module.meta.tier).collect();
        t.dedup();
        t
    };
    let body = html! {
        section.wrap.page-head {
            h1 { "Learn" }
            p.page-lead {
                "From your first packet capture to a 5/5 daily. Each module has the idea, a "
                "walkthrough, a practice puzzle and a checkpoint."
            }
        }
        section.wrap.section-tight #learn data-passed=(passed.iter().cloned().collect::<Vec<_>>().join(",")) {
            (path_form(path, "/learn"))
            @if path.is_none() {
                p.muted { "Choose how you want to work. You can switch at any time; progress is kept either way." }
            }
            @for t in &tiers {
                @let in_tier: Vec<&LearnModule> = modules.iter().filter(|m| m.module.meta.tier == *t).collect();
                @let done = in_tier.iter().all(|m| passed.contains(m.id()));
                section.tier data-tier=(t) {
                    h2 {
                        (tier_name(*t))
                        " "
                        span.badge.tier-badge.hidden[!done] { "Complete" }
                    }
                    ol.module-list {
                        @for m in &in_tier {
                            @let meta = &m.module.meta;
                            li.module-row {
                                a.module-link href={"/learn/" (meta.id)} data-module=(meta.id) {
                                    span.module-num { (meta.number) }
                                    span.module-text {
                                        span.module-title { (meta.title) }
                                        span.module-summary { (meta.summary) }
                                    }
                                    span.passed-mark.hidden[!passed.contains(&meta.id)] { "Passed" }
                                }
                            }
                        }
                    }
                }
            }
            h2.learn-practice-head { "All practice puzzles" }
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
        "Malware analysis from a first packet capture to a 5/5 puzzle: pcaps, HTTP and C2, curl, encodings, crypto, binaries, scripting and live C2 servers.",
        Nav::Learn,
        "/learn",
        body,
    )
}

// ---------------------------------------------------------------- module page

#[derive(Deserialize, Default)]
pub struct PathQuery {
    #[serde(default)]
    path: String,
}

/// Dailies that have closed and name this module.
fn worked_examples<'a>(state: &'a AppState, id: &str) -> Vec<&'a RenderedPack> {
    state
        .released_dailies()
        .into_iter()
        .filter(|p| state.is_closed(p) && p.pack.meta.learn.iter().any(|l| l == id))
        .rev()
        .collect()
}

struct Graded<'a> {
    answers: &'a [String],
    results: Vec<bool>,
}

fn module_page(
    state: &AppState,
    viewer: Option<&User>,
    m: &LearnModule,
    // The walkthrough to show, and whether the player chose it.
    (path, chosen): (ToolPath, bool),
    passed: bool,
    graded: Option<Graded>,
    stats: &Stats,
) -> Markup {
    let meta = &m.module.meta;
    let pos = state
        .learn
        .iter()
        .position(|x| x.id() == meta.id)
        .unwrap_or(0);
    let prev = pos.checked_sub(1).and_then(|i| state.learn.get(i));
    let next = state.learn.get(pos + 1);
    let here = format!("/learn/{}", meta.id);
    let other = match path {
        ToolPath::Browser => ToolPath::Terminal,
        ToolPath::Terminal => ToolPath::Browser,
    };
    let practice: Vec<(&String, Option<&RenderedPack>)> =
        meta.practice.iter().map(|id| (id, state.get(id))).collect();
    let more: Vec<&RenderedPack> = meta.more.iter().filter_map(|id| state.get(id)).collect();
    let examples = worked_examples(state, &meta.id);
    let all_right = graded
        .as_ref()
        .is_some_and(|g| g.results.iter().all(|r| *r));

    let body = html! {
        div.wrap.learn-module #learn-module data-module=(meta.id) data-passed[passed || all_right] {
            section.page-head {
                p.crumb {
                    a href="/learn" { "Learn" } " · Tier " (meta.tier) " · Module " (meta.number) " of " (state.learn.len())
                }
                h1 { (meta.title) }
                p.page-lead { (meta.summary) }
                p.notice.passed-note.hidden[!(passed || all_right)] { "Checkpoint passed." }
            }
            section.learn-section aria-labelledby="concept-h" {
                h2 #concept-h { "The idea" }
                div.prose { (PreEscaped(&m.concept_html)) }
            }
            section.learn-section aria-labelledby="walk-h" {
                h2 #walk-h {
                    @match path {
                        ToolPath::Browser => "Walkthrough: in the browser",
                        ToolPath::Terminal => "Walkthrough: in a terminal",
                    }
                }
                form.path-switch method="post" action="/learn/path" {
                    input type="hidden" name="back" value=(here);
                    @if !chosen { span.muted { "Showing the browser path. " } }
                    button.link-button type="submit" name="path" value=(other.as_str()) {
                        @match other {
                            ToolPath::Browser => "Switch to the browser path",
                            ToolPath::Terminal => "Switch to the terminal path",
                        }
                    }
                }
                div.prose {
                    @match path {
                        ToolPath::Browser => (PreEscaped(&m.browser_html)),
                        ToolPath::Terminal => (PreEscaped(&m.terminal_html)),
                    }
                }
            }
            section.learn-section aria-labelledby="practice-h" {
                h2 #practice-h { "Practice" }
                ul.work-list {
                    @for (id, p) in &practice {
                        @if let Some(p) = p {
                            (pack_row(state, p, stats))
                        } @else {
                            li.work-row.muted {
                                (id) ": "
                                @if state.get_any(id).is_none() {
                                    "this puzzle needs live puzzles or the range, which this site has not turned on."
                                } @else {
                                    "not available yet."
                                }
                            }
                        }
                    }
                }
                @if !more.is_empty() {
                    h3 { "More at this level" }
                    ul.work-list {
                        @for p in &more { (pack_row(state, p, stats)) }
                    }
                }
                @if !examples.is_empty() {
                    h3 { "Worked examples" }
                    p.muted { "Past dailies that use this module, with their explainers once solved." }
                    ul.work-list {
                        @for p in &examples { (pack_row(state, p, stats)) }
                    }
                }
            }
            section.learn-section #checkpoint aria-labelledby="check-h" {
                h2 #check-h { "Checkpoint" }
                p.muted { "Answers from either path count." }
                form.checkpoint method="post" action={(here) "/checkpoint#checkpoint"} {
                    @for (i, q) in meta.checkpoint.iter().enumerate() {
                        @let answer = graded.as_ref().and_then(|g| g.answers.get(i)).map(String::as_str).unwrap_or("");
                        @let result = graded.as_ref().and_then(|g| g.results.get(i)).copied();
                        div.question {
                            label for={"q" (i)} { (i + 1) ". " (q.question) }
                            @if q.kind == Kind::Curl {
                                textarea id={"q" (i)} name={"a" (i)} rows="3" autocomplete="off"
                                    autocapitalize="off" spellcheck="false" maxlength="2048" { (answer) }
                            } @else {
                                input id={"q" (i)} name={"a" (i)} type="text" value=(answer) autocomplete="off"
                                    autocapitalize="off" spellcheck="false" maxlength="2048";
                            }
                            @match result {
                                Some(true) => p.answer-msg.ok-text { "Right." },
                                Some(false) => p.answer-msg.err-text {
                                    "Not it."
                                    @if let Some(h) = &q.hint { " " (h) }
                                },
                                None => {},
                            }
                        }
                    }
                    button.button type="submit" { "Check answers" }
                    @if graded.is_some() && all_right && viewer.is_none() {
                        p.muted { "Passed. Kept in this browser; " a href="/login" { "log in" } " to keep it on your account." }
                    }
                }
            }
            nav.pager aria-label="Modules" {
                @if let Some(p) = prev {
                    a href={"/learn/" (p.id())} { "← " (p.module.meta.title) }
                } @else { a href="/learn" { "← Learn" } }
                @if let Some(n) = next {
                    a href={"/learn/" (n.id())} { (n.module.meta.title) " →" }
                } @else if state.today_daily().is_some() {
                    a href="/" { "Today's puzzle →" }
                }
            }
        }
    };
    layout(
        state,
        viewer,
        &meta.title,
        &meta.summary,
        Nav::Learn,
        &here,
        body,
    )
}

/// `GET /learn/{id}`. `?path=browser|terminal` shows that walkthrough once,
/// without changing the saved choice.
pub async fn module(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    Path(id): Path<String>,
    Query(q): Query<PathQuery>,
    headers: HeaderMap,
) -> Response {
    let Some(m) = state.learn.iter().find(|m| m.id() == id) else {
        return not_found_page(&state, viewer.as_ref()).into_response();
    };
    let chosen = chosen_path(&state, viewer.as_ref(), &headers).await;
    let path = ToolPath::parse(&q.path)
        .or(chosen)
        .unwrap_or(ToolPath::Browser);
    let passed = passed_of(&state, viewer.as_ref()).await.contains(&id);
    let stats = state.stats().await;
    module_page(
        &state,
        viewer.as_ref(),
        m,
        (path, chosen.is_some() || ToolPath::parse(&q.path).is_some()),
        passed,
        None,
        &stats,
    )
    .into_response()
}

// ---------------------------------------------------------------- checkpoint

/// Grades answers and records a pass for a signed-in player. Returns the
/// per-question results.
async fn grade_and_record(
    state: &AppState,
    viewer: Option<&User>,
    m: &LearnModule,
    answers: &[String],
) -> Vec<bool> {
    let results = m.module.grade(answers);
    if results.iter().all(|r| *r) {
        if let Some(u) = viewer {
            if let Err(e) = state.db.learn_pass(u.id, m.id().to_owned()).await {
                tracing::error!("recording a checkpoint failed: {e}");
            }
        }
    }
    results
}

/// The form's fields `a0`, `a1`, ... in order.
fn answers_from_form(fields: &[(String, String)], n: usize) -> Vec<String> {
    (0..n)
        .map(|i| {
            let key = format!("a{i}");
            fields
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.chars().take(2048).collect())
                .unwrap_or_default()
        })
        .collect()
}

/// `POST /learn/{id}/checkpoint`, the plain form.
pub async fn checkpoint_form(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    Path(id): Path<String>,
    headers: HeaderMap,
    Form(fields): Form<Vec<(String, String)>>,
) -> Response {
    let Some(m) = state.learn.iter().find(|m| m.id() == id) else {
        return not_found_page(&state, viewer.as_ref()).into_response();
    };
    let answers = answers_from_form(&fields, m.module.meta.checkpoint.len());
    let results = grade_and_record(&state, viewer.as_ref(), m, &answers).await;
    let chosen = chosen_path(&state, viewer.as_ref(), &headers).await;
    let passed = passed_of(&state, viewer.as_ref()).await.contains(&id);
    let stats = state.stats().await;
    module_page(
        &state,
        viewer.as_ref(),
        m,
        (chosen.unwrap_or(ToolPath::Browser), chosen.is_some()),
        passed,
        Some(Graded {
            answers: &answers,
            results,
        }),
        &stats,
    )
    .into_response()
}

#[derive(Deserialize)]
pub struct CheckpointReq {
    answers: Vec<String>,
}

#[derive(Serialize)]
pub struct CheckpointRes {
    passed: bool,
    results: Vec<bool>,
}

/// `POST /api/learn/{id}/checkpoint` `{"answers": [...]}`. app.js uses it to
/// send checkpoints passed while signed out, after signing in.
pub async fn checkpoint_api(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    Path(id): Path<String>,
    Json(req): Json<CheckpointReq>,
) -> Response {
    let Some(m) = state.learn.iter().find(|m| m.id() == id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let answers: Vec<String> = req
        .answers
        .into_iter()
        .take(m.module.meta.checkpoint.len())
        .collect();
    let results = grade_and_record(&state, viewer.as_ref(), m, &answers).await;
    Json(CheckpointRes {
        passed: results.iter().all(|r| *r),
        results,
    })
    .into_response()
}

#[derive(Serialize)]
pub struct LearnProgress {
    user: Option<String>,
    path: Option<&'static str>,
    passed: Vec<String>,
    modules: Vec<String>,
}

/// `GET /api/me/learn`.
pub async fn progress(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    headers: HeaderMap,
) -> Json<LearnProgress> {
    let path = chosen_path(&state, viewer.as_ref(), &headers).await;
    let passed = passed_of(&state, viewer.as_ref()).await;
    Json(LearnProgress {
        user: viewer.map(|u| u.username),
        path: path.map(ToolPath::as_str),
        passed: state
            .learn
            .iter()
            .filter(|m| passed.contains(m.id()))
            .map(|m| m.id().to_owned())
            .collect(),
        modules: state.learn.iter().map(|m| m.id().to_owned()).collect(),
    })
}

// ---------------------------------------------------------------- path

#[derive(Deserialize)]
pub struct PathForm {
    path: String,
    #[serde(default)]
    back: String,
}

/// `POST /learn/path`: saves the tool path on the account (when signed in) and
/// in a cookie, then goes back to the Learn page it came from.
pub async fn set_path(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    Form(f): Form<PathForm>,
) -> Response {
    let Some(path) = ToolPath::parse(&f.path) else {
        return (StatusCode::BAD_REQUEST, "Choose browser or terminal.").into_response();
    };
    if let Some(u) = &viewer {
        if let Err(e) = state
            .db
            .set_learn_path(u.id, path.as_str().to_owned())
            .await
        {
            tracing::error!("saving the learn path failed: {e}");
        }
    }
    // Only back to a Learn page of this site.
    let back = if f.back == "/learn"
        || (f.back.starts_with("/learn/")
            && f.back[7..]
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'))
    {
        f.back
    } else {
        "/learn".to_owned()
    };
    let cookie = format!(
        "{PATH_COOKIE}={}; Path=/; Max-Age=31536000; SameSite=Lax{}",
        path.as_str(),
        if state.secure_cookies { "; Secure" } else { "" }
    );
    ([(header::SET_COOKIE, cookie)], Redirect::to(&back)).into_response()
}
