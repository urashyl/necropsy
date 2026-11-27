// SPDX-License-Identifier: AGPL-3.0-or-later
//! The in-browser toolkit's third-party builds (Wiregasm, Pyodide, CyberChef),
//! served from `NECROPSY_TOOLKIT` (default `toolkit/dist`, filled by
//! `toolkit/fetch.sh`). See `docs/toolkit.md`.
//!
//! Files are looked up in a list made at startup, never joined onto a path
//! from the request, and streamed from disk: the largest is over 70 MB. Each
//! directory is named `<name>-<version>`, so a URL never changes its content
//! and can be cached for good.
//!
//! CyberChef runs in a sandboxed frame: its pages get a CSP `sandbox` without
//! `allow-same-origin`, so it runs in an opaque origin with no access to the
//! site's cookies, storage or API.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::extract::{Path as UrlPath, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use std::sync::Arc;

use crate::state::AppState;

/// The tools the workspace uses, by the name in `toolkit/sources.txt`.
pub const TOOLS: [&str; 3] = ["wiregasm", "pyodide", "cyberchef"];

/// CSP for CyberChef's pages and files. `sandbox` without `allow-same-origin`
/// gives the frame an opaque origin. It needs inline scripts and `eval`, which
/// is why it is kept out of the site's origin.
const CYBERCHEF_CSP: &str =
    "sandbox allow-scripts allow-downloads allow-popups allow-modals allow-forms; \
     default-src 'self'; script-src 'self' 'unsafe-inline' 'unsafe-eval' 'wasm-unsafe-eval' blob:; \
     style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; \
     worker-src 'self' blob:; connect-src 'self' data: blob:; form-action 'none'; base-uri 'self'; \
     frame-ancestors 'self'";

/// CSP for every other toolkit file. They are scripts, WebAssembly and data
/// loaded by the workspace's workers, never documents.
const FILE_CSP: &str = "default-src 'none'; sandbox; frame-ancestors 'none'";

#[derive(Clone)]
struct Entry {
    disk: PathBuf,
    /// A `.gz` copy exists next to it.
    gz: bool,
}

/// The toolkit files found at startup.
#[derive(Default)]
pub struct Toolkit {
    files: HashMap<String, Entry>,
    /// `wiregasm` -> `wiregasm-2.0.0`.
    dirs: HashMap<String, String>,
}

impl Toolkit {
    /// Lists `root/<name>-<version>/...`. A missing directory gives an empty
    /// toolkit: the site runs and the workspace says the tools are not installed.
    pub fn load(root: &Path) -> Self {
        let mut tk = Toolkit::default();
        let Ok(entries) = std::fs::read_dir(root) else {
            return tk;
        };
        for e in entries.flatten() {
            let Some(dir) = e.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Some(tool) = TOOLS
                .iter()
                .find(|t| dir.strip_prefix(**t).is_some_and(|r| r.starts_with('-')))
            else {
                continue;
            };
            if !e.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            // Two versions of one tool: keep the newest name, so a stale copy
            // left in the directory does not win.
            let keep = tk
                .dirs
                .get(*tool)
                .is_none_or(|old| old.as_str() < dir.as_str());
            walk(&e.path(), &dir, &mut tk.files);
            if keep {
                tk.dirs.insert((*tool).to_owned(), dir);
            }
        }
        tk
    }

    /// The URL prefix of a tool, like `/toolkit/wiregasm-2.0.0`, if installed.
    pub fn base(&self, tool: &str) -> Option<String> {
        self.dirs.get(tool).map(|d| format!("/toolkit/{d}"))
    }

    /// Whether every tool is installed.
    pub fn complete(&self) -> bool {
        TOOLS.iter().all(|t| self.dirs.contains_key(*t))
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }
}

fn walk(dir: &Path, prefix: &str, out: &mut HashMap<String, Entry>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let Some(name) = e.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Ok(ft) = e.file_type() else { continue };
        let rel = format!("{prefix}/{name}");
        if ft.is_dir() {
            walk(&e.path(), &rel, out);
        } else if ft.is_file() && !name.ends_with(".gz") {
            let mut gz = e.path().into_os_string();
            gz.push(".gz");
            out.insert(
                rel,
                Entry {
                    disk: e.path(),
                    gz: Path::new(&gz).is_file(),
                },
            );
        }
    }
}

fn content_type(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "wasm" => "application/wasm",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "ttf" => "font/ttf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "txt" | "fnt" => "text/plain; charset=utf-8",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

fn accepts_gzip(headers: &HeaderMap) -> bool {
    headers
        .get_all(header::ACCEPT_ENCODING)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|e| {
            let mut parts = e.split(';');
            parts.next().is_some_and(|n| n.trim() == "gzip")
                && !parts.any(|q| q.trim().replace(' ', "") == "q=0")
        })
}

/// `GET /toolkit/{*path}`.
pub async fn file(
    State(state): State<Arc<AppState>>,
    UrlPath(path): UrlPath<String>,
    headers: HeaderMap,
) -> Response {
    let Some(entry) = state.toolkit.files.get(&path).cloned() else {
        return (StatusCode::NOT_FOUND, "Not found.").into_response();
    };
    let gzip = entry.gz && accepts_gzip(&headers);
    let disk = if gzip {
        let mut p = entry.disk.clone().into_os_string();
        p.push(".gz");
        PathBuf::from(p)
    } else {
        entry.disk.clone()
    };
    let f = match tokio::fs::File::open(&disk).await {
        Ok(f) => f,
        Err(e) => {
            tracing::error!("toolkit file {} cannot be read: {e}", disk.display());
            return (StatusCode::INTERNAL_SERVER_ERROR, "Server error.").into_response();
        }
    };
    let len = f.metadata().await.map(|m| m.len()).ok();
    let mut res = Response::new(Body::from_stream(tokio_util::io::ReaderStream::new(f)));
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(content_type(&path)),
    );
    if let Some(len) = len {
        h.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    }
    if gzip {
        h.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    }
    h.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    if path.starts_with("cyberchef-") {
        h.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(CYBERCHEF_CSP),
        );
        h.insert(
            header::X_FRAME_OPTIONS,
            HeaderValue::from_static("SAMEORIGIN"),
        );
        // The sandboxed frame's origin is opaque, so its own fonts and scripts
        // are cross-origin to it. These files are public.
        h.insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_static("*"),
        );
    } else {
        h.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(FILE_CSP),
        );
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gzip_negotiation() {
        let mut h = HeaderMap::new();
        assert!(!accepts_gzip(&h));
        h.insert(
            header::ACCEPT_ENCODING,
            "gzip, deflate, br".parse().unwrap(),
        );
        assert!(accepts_gzip(&h));
        h.insert(header::ACCEPT_ENCODING, "br, gzip;q=0".parse().unwrap());
        assert!(!accepts_gzip(&h));
        h.insert(header::ACCEPT_ENCODING, "gzipx".parse().unwrap());
        assert!(!accepts_gzip(&h));
    }

    #[test]
    fn load_lists_versioned_tool_directories_only() {
        let root = std::env::temp_dir().join(format!("necropsy-tk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("wiregasm-2.0.0/sub")).unwrap();
        std::fs::create_dir_all(root.join("unknown-1.0")).unwrap();
        std::fs::write(root.join("wiregasm-2.0.0/wiregasm.js"), "x").unwrap();
        std::fs::write(root.join("wiregasm-2.0.0/wiregasm.js.gz"), "x").unwrap();
        std::fs::write(root.join("wiregasm-2.0.0/sub/a.data"), "x").unwrap();
        std::fs::write(root.join("unknown-1.0/x.js"), "x").unwrap();
        let tk = Toolkit::load(&root);
        assert_eq!(
            tk.base("wiregasm").as_deref(),
            Some("/toolkit/wiregasm-2.0.0")
        );
        assert!(tk.base("pyodide").is_none());
        assert!(!tk.complete());
        assert!(tk.files["wiregasm-2.0.0/wiregasm.js"].gz);
        assert!(!tk.files["wiregasm-2.0.0/sub/a.data"].gz);
        assert!(!tk.files.contains_key("wiregasm-2.0.0/wiregasm.js.gz"));
        assert!(!tk.files.keys().any(|k| k.starts_with("unknown")));
        assert!(Toolkit::load(&root.join("missing")).files.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
