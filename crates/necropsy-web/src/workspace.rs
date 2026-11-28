// SPDX-License-Identifier: AGPL-3.0-or-later
//! The workspace: one page per puzzle with the in-browser toolkit, so a
//! difficulty 1-3 puzzle can be solved with nothing installed. Panes: files,
//! pcap viewer (Wiregasm), decoder (CyberChef), request console (live puzzles),
//! notebook (Pyodide) and hex/strings viewer. See `docs/toolkit.md`.
//!
//! The toolkit's own scripts are embedded here and served from
//! `/static/toolkit/`. Its workers run WebAssembly, so they are served with a
//! CSP of their own that allows it; the page keeps the site's policy plus a
//! frame for the decoder.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use maud::{html, Markup, PreEscaped};

use crate::auth::MaybeUser;
use crate::pages::{
    answer_block, crumb, layout, live_panel, not_found_page, puzzle_url, release_day, side_facts,
    viewer_files, with_cookie, Nav, ViewerFiles,
};
use crate::state::{short_hash, AppState};

/// The toolkit's scripts, by file name.
pub const FILES: [(&str, &str); 10] = [
    (
        "workspace.js",
        include_str!("../static/toolkit/workspace.js"),
    ),
    ("curl.js", include_str!("../static/toolkit/curl.js")),
    ("console.js", include_str!("../static/toolkit/console.js")),
    ("pcap.js", include_str!("../static/toolkit/pcap.js")),
    (
        "pcap-worker.js",
        include_str!("../static/toolkit/pcap-worker.js"),
    ),
    ("decoder.js", include_str!("../static/toolkit/decoder.js")),
    ("notebook.js", include_str!("../static/toolkit/notebook.js")),
    (
        "py-worker.js",
        include_str!("../static/toolkit/py-worker.js"),
    ),
    ("hex.js", include_str!("../static/toolkit/hex.js")),
    ("necropsy.py", include_str!("../static/toolkit/necropsy.py")),
];

/// The page's policy: the site's own, plus the decoder's frame and the
/// workers. `connect-src 'self'` covers the console's WebSocket.
const PAGE_CSP: &str = "default-src 'none'; script-src 'self' https://challenges.cloudflare.com; \
     style-src 'self' https://fonts.googleapis.com; font-src https://fonts.gstatic.com; \
     img-src 'self' data:; connect-src 'self' https://challenges.cloudflare.com; \
     frame-src 'self' https://challenges.cloudflare.com; worker-src 'self'; base-uri 'none'; \
     form-action 'self'; frame-ancestors 'none'";

/// A worker's policy. Wiregasm's bindings are built with `new Function`, so the
/// pcap worker needs `unsafe-eval`; both need `wasm-unsafe-eval`. A worker has
/// no DOM, and loads nothing but the site's own files.
const WORKER_CSP: &str = "default-src 'none'; script-src 'self' 'unsafe-eval' 'wasm-unsafe-eval'; \
     connect-src 'self'";

/// `GET /static/toolkit/{file}`. Revalidated by ETag on every load, so a module
/// imported by another always matches it after a deploy.
pub async fn static_file(Path(file): Path<String>, headers: HeaderMap) -> Response {
    let Some((name, body)) = FILES.iter().find(|(n, _)| *n == file) else {
        return (StatusCode::NOT_FOUND, "Not found.").into_response();
    };
    let etag = format!("\"{}\"", short_hash(body));
    let fresh = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|t| t.trim() == etag));
    let ctype = if name.ends_with(".py") {
        "text/plain; charset=utf-8"
    } else {
        "text/javascript; charset=utf-8"
    };
    let mut res = if fresh {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        ([(header::CONTENT_TYPE, ctype)], *body).into_response()
    };
    let h = res.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    if let Ok(v) = HeaderValue::from_str(&etag) {
        h.insert(header::ETAG, v);
    }
    if name.ends_with("-worker.js") {
        h.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(WORKER_CSP),
        );
    }
    res
}

/// How a file opens in the workspace.
fn file_kind(path: &str, text: bool) -> &'static str {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".pcap") || lower.ends_with(".pcapng") || lower.ends_with(".cap") {
        "pcap"
    } else if text {
        "text"
    } else {
        "binary"
    }
}

/// The files, for the page's script: path, size and kind.
fn files_json(files: &[necropsy_pack::PackFile]) -> String {
    let list: Vec<serde_json::Value> = files
        .iter()
        .map(|f| {
            serde_json::json!({
                "path": f.path,
                "size": f.data.len(),
                "kind": file_kind(&f.path, f.text().is_some()),
            })
        })
        .collect();
    serde_json::Value::Array(list).to_string()
}

/// `GET /w/{id}`.
pub async fn page(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let Some(rp) = state.get(&id) else {
        return not_found_page(&state, viewer.as_ref()).into_response();
    };
    let (files, cookie) = viewer_files(&state, viewer.as_ref(), &headers, rp).await;
    let raw_files: &[necropsy_pack::PackFile] = match &files {
        ViewerFiles::Pack(_) => &rp.pack.files,
        ViewerFiles::Generated(g) => &g.output.files,
        ViewerFiles::Failed => &[],
    };
    let stats = state.stats().await;
    let ranked = crate::leaderboard::ranked_status_for(&state, viewer.as_ref(), rp).await;
    let m = &rp.pack.meta;
    let live = rp.pack.is_live();
    let tk = &state.toolkit;
    let js_v = short_hash(FILES[0].1);
    let tabs: Vec<(&str, &str)> = [
        ("files", "Files"),
        ("pcap", "Pcap"),
        ("decoder", "Decoder"),
        ("console", "Console"),
        ("notebook", "Notebook"),
        ("hex", "Hex"),
    ]
    .into_iter()
    .filter(|(t, _)| *t != "console" || live)
    .collect();

    let body = html! {
        div #puzzle.workspace data-id=(m.id) data-number=(m.number) data-title=(m.title)
            data-hints=(m.hints.len()) data-url=(puzzle_url(&state, rp))
            data-label=(rp.label().to_lowercase()) data-day=(release_day(rp))
            data-files=(files_json(raw_files)) data-live=(live)
            data-signed-in=(viewer.is_some())
            data-wiregasm=[tk.base("wiregasm")] data-pyodide=[tk.base("pyodide")]
            data-cyberchef=[tk.base("cyberchef")] {
            section.wrap-wide.page-head {
                (crumb(&state, rp))
                h1 { (m.title) }
                p.muted { a href={"/p/" (m.id)} { "← Puzzle page" } " · " a href="/learn" { "How to use these tools" } }
            }
            div.wrap-wide.ws-grid {
                div.ws-main {
                    details.ws-brief open {
                        summary { "Brief" }
                        div.prose.brief { (PreEscaped(&rp.brief_html)) }
                    }
                    @if live {
                        (live_panel(&state, viewer.as_ref(), rp, ranked.as_deref()))
                    }
                    div.ws-tabs role="tablist" aria-label="Tools" {
                        @for (i, (t, label)) in tabs.iter().enumerate() {
                            button.ws-tab type="button" role="tab" id={"tab-" (t)}
                                aria-controls={"pane-" (t)} aria-selected=(i == 0)
                                tabindex=(if i == 0 { "0" } else { "-1" }) data-tool=(t) { (label) }
                        }
                    }
                    (panes(live))
                    noscript { p.notice { "The workspace needs JavaScript." } }
                }
                aside.ws-side {
                    (answer_block(rp))
                    (side_facts(rp, &stats, raw_files.len()))
                }
            }
        }
        script type="module" src={"/static/toolkit/workspace.js?v=" (js_v)} {}
    };
    let mut res = layout(
        &state,
        viewer.as_ref(),
        &format!("Workspace: {}", m.title),
        &format!("Tools for necropsy puzzle #{}: {}", m.number, m.title),
        Nav::None,
        &format!("/w/{}", m.id),
        body,
    )
    .into_response();
    res.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(PAGE_CSP),
    );
    with_cookie(res, cookie)
}

/// One "Terminal:" line, filled in by the tool's script.
fn cli(id: &str, initial: &str) -> Markup {
    html! {
        p.cli { span.cli-label { "Terminal: " } code id=(id) { (initial) } }
    }
}

/// The panes' skeletons. Each tool's script fills its own.
fn panes(live: bool) -> Markup {
    html! {
        section #pane-files.ws-pane role="tabpanel" aria-labelledby="tab-files" data-tool="files" {
            ul #files-list.files-list {}
            div #file-view.file-view.hidden {
                div.tool-row {
                    span #file-view-name.file-view-name {}
                    button #file-view-close.link-button type="button" { "Close" }
                }
                pre #file-view-body.raw-view tabindex="0" role="region" aria-label="File contents" {}
            }
            (cli("cli-files", "curl -O <file URL>"))
        }
        section #pane-pcap.ws-pane.hidden role="tabpanel" aria-labelledby="tab-pcap" data-tool="pcap" {
            div.tool-row {
                label for="pcap-file" { "Capture" }
                select #pcap-file {}
            }
            form #pcap-filter-form.tool-row {
                label for="pcap-filter" { "Display filter" }
                input #pcap-filter type="text" placeholder="http.request" autocomplete="off"
                    autocapitalize="off" spellcheck="false";
                button.button type="submit" { "Apply" }
            }
            p #pcap-msg.tool-msg role="status" aria-live="polite" {}
            div.table-scroll tabindex="0" role="region" aria-label="Packets" {
                table #pcap-table.packet-table {
                    thead { tr { th { "No." } th { "Time" } th { "Source" } th { "Destination" } th { "Protocol" } th { "Length" } th { "Info" } } }
                    tbody {}
                }
            }
            div #pcap-detail.pcap-detail.hidden {
                div.tool-row {
                    button #pcap-follow-tcp.button.button-quiet type="button" { "Follow TCP stream" }
                    button #pcap-follow-http.button.button-quiet type="button" { "Follow HTTP stream" }
                    button #pcap-to-decoder.link-button type="button" { "Send stream to decoder" }
                }
                pre #pcap-stream.stream-view.hidden tabindex="0" {}
                div #pcap-tree.proto-tree {}
            }
            (cli("cli-pcap", "tshark -r capture.pcap"))
        }
        section #pane-decoder.ws-pane.hidden role="tabpanel" aria-labelledby="tab-decoder" data-tool="decoder" {
            p #decoder-msg.tool-msg role="status" aria-live="polite" {}
            div #decoder-frame.decoder-frame {}
            (cli("cli-decoder", "base64 -d, xxd -r -p, python3"))
        }
        @if live {
            section #pane-console.ws-pane.hidden role="tabpanel" aria-labelledby="tab-console" data-tool="console" {
                div.tool-row {
                    span #console-target.muted { "Start the instance to send requests." }
                    label.check { input #console-blocks type="checkbox"; " Block mode" }
                }
                div #console-builder.block-builder.hidden {}
                form #console-form.console-form {
                    label for="console-cmd" { "Command" }
                    textarea #console-cmd rows="4" autocomplete="off" autocapitalize="off"
                        spellcheck="false" placeholder="curl -i https://<instance>/" {}
                    div.tool-row {
                        button.button type="submit" { "Send" }
                        span #console-msg.tool-msg role="status" aria-live="polite" {}
                    }
                }
                div.console-out {
                    h3 { "Terminal output" }
                    pre #console-term.raw-view tabindex="0" {}
                    h3 { "Request sent" }
                    pre #console-request.raw-view tabindex="0" {}
                    h3 { "Response received" }
                    pre #console-response.raw-view tabindex="0" {}
                }
            }
        }
        section #pane-notebook.ws-pane.hidden role="tabpanel" aria-labelledby="tab-notebook" data-tool="notebook" {
            p #notebook-msg.tool-msg role="status" aria-live="polite" {}
            div #notebook-cells.notebook {}
            div.tool-row {
                button #notebook-add.button.button-quiet type="button" { "Add cell" }
                button #notebook-run-all.button.button-quiet type="button" { "Run all" }
            }
            (cli("cli-notebook", "python3 script.py"))
        }
        section #pane-hex.ws-pane.hidden role="tabpanel" aria-labelledby="tab-hex" data-tool="hex" {
            div.tool-row {
                label for="hex-file" { "File" }
                select #hex-file {}
                label for="hex-mode" { "View" }
                select #hex-mode {
                    option value="hex" { "Hex" }
                    option value="strings" { "Strings" }
                }
            }
            form #hex-jump-form.tool-row {
                label for="hex-offset" { "Offset" }
                input #hex-offset type="text" inputmode="text" placeholder="0x0" autocomplete="off";
                label for="hex-min" { "Min length" }
                input #hex-min type="number" min="2" max="64" value="6";
                button.button.button-quiet type="submit" { "Go" }
            }
            p #hex-msg.tool-msg role="status" aria-live="polite" {}
            pre #hex-view.hex-view tabindex="0" {}
            button #hex-more.link-button.hidden type="button" { "Show more" }
            (cli("cli-hex", "xxd file | less"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_kinds() {
        assert_eq!(file_kind("a/capture.PCAP", false), "pcap");
        assert_eq!(file_kind("x.pcapng", false), "pcap");
        assert_eq!(file_kind("strings.txt", true), "text");
        assert_eq!(file_kind("blob.bin", false), "binary");
    }

    #[test]
    fn worker_files_are_the_ones_with_the_worker_policy() {
        let workers: Vec<&str> = FILES
            .iter()
            .map(|(n, _)| *n)
            .filter(|n| n.ends_with("-worker.js"))
            .collect();
        assert_eq!(workers, ["pcap-worker.js", "py-worker.js"]);
    }
}
