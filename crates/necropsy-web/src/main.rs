//! necropsy web front end.
//!
//! Server-rendered pages over a directory of packs (see `docs/web.md`).
//! There are no accounts. The server keeps no per-player state: solves,
//! streaks and timers live in the browser's localStorage. The only thing the
//! server decides is whether a submitted answer is right, so the explainer is
//! not sent until it is.

mod markdown;
mod pages;
mod state;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tower_http::compression::CompressionLayer;
use tower_http::trace::TraceLayer;

use crate::state::AppState;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "necropsy_web=info,tower_http=info".into()),
        )
        .init();

    let addr: SocketAddr = env_or("NECROPSY_ADDR", "127.0.0.1:8080")
        .parse()
        .expect("NECROPSY_ADDR must be host:port");
    let packs_dir = PathBuf::from(env_or("NECROPSY_PACKS", "packs"));
    let base_url = env_or("NECROPSY_BASE_URL", "https://necropsy.urashyl.dev");

    let packs = match necropsy_pack::load_all(&packs_dir) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("cannot load packs from {}: {e}", packs_dir.display());
            std::process::exit(1);
        }
    };
    tracing::info!("loaded {} packs from {}", packs.len(), packs_dir.display());

    let state = Arc::new(AppState::new(packs, base_url));
    let app = router(state);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| {
            eprintln!("cannot bind {addr}: {e}");
            std::process::exit(1);
        });
    tracing::info!("listening on http://{addr}");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .expect("server error");
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_owned())
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(pages::today))
        .route("/p/{id}", get(pages::puzzle))
        .route("/p/{id}/files/{*path}", get(pages::raw_file))
        .route("/archive", get(pages::archive))
        .route("/format", get(pages::format))
        .route(
            "/api/p/{id}/answer",
            // Answers are short; a small cap keeps the endpoint cheap.
            post(check_answer).layer(DefaultBodyLimit::max(2 * 1024)),
        )
        .route("/healthz", get(|| async { "ok" }))
        .route("/static/app.css", get(pages::static_css))
        .route("/static/app.js", get(pages::static_js))
        .route("/static/theme.js", get(pages::static_theme_js))
        .route("/favicon.svg", get(pages::favicon))
        .fallback(pages::not_found)
        .layer(middleware::from_fn(security_headers))
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Headers for every response. The CSP allows only our own scripts plus the
/// Google Fonts stylesheet and font files, which is why no page has an inline
/// script or inline style.
async fn security_headers(req: axum::extract::Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; script-src 'self'; style-src 'self' https://fonts.googleapis.com; \
             font-src https://fonts.gstatic.com; img-src 'self' data:; connect-src 'self'; \
             base-uri 'none'; form-action 'self'; frame-ancestors 'none'",
        ),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    res
}

#[derive(Deserialize)]
struct AnswerReq {
    answer: String,
}

#[derive(Serialize)]
struct AnswerRes {
    correct: bool,
    /// Rendered explainer HTML, only present when `correct`.
    #[serde(skip_serializing_if = "Option::is_none")]
    explainer_html: Option<String>,
}

async fn check_answer(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<AnswerReq>,
) -> Response {
    let Some(p) = state.get(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    // No answer is anywhere near this long, so skip hashing it.
    if req.answer.len() > 512 {
        return Json(AnswerRes {
            correct: false,
            explainer_html: None,
        })
        .into_response();
    }
    let correct = p.pack.check(&req.answer);
    Json(AnswerRes {
        correct,
        explainer_html: correct.then(|| p.explainer_html.clone()),
    })
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn app() -> Router {
        let packs = necropsy_pack::load_all(std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../packs"
        )))
        .unwrap();
        router(Arc::new(AppState::new(
            packs,
            "https://example.test".into(),
        )))
    }

    async fn get_body(path: &str) -> (StatusCode, String) {
        let res = app()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    async fn post_answer(id: &str, answer: &str) -> (StatusCode, String) {
        let body = serde_json_lite(answer);
        let res = app()
            .oneshot(
                Request::post(format!("/api/p/{id}/answer"))
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    fn serde_json_lite(answer: &str) -> String {
        format!(
            "{{\"answer\":\"{}\"}}",
            answer.replace('\\', "\\\\").replace('"', "\\\"")
        )
    }

    #[tokio::test]
    async fn pages_render() {
        for path in ["/", "/archive", "/format", "/p/phone-home", "/p/peanuts"] {
            let (status, body) = get_body(path).await;
            assert_eq!(status, StatusCode::OK, "{path}");
            assert!(body.contains("<title>"), "{path}");
        }
        assert_eq!(get_body("/p/nope").await.0, StatusCode::NOT_FOUND);
        assert_eq!(get_body("/nope").await.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn puzzle_page_never_contains_explainer_or_answer() {
        let (_, body) = get_body("/p/phone-home").await;
        assert!(!body.contains("sync.pagepilot-cdn.invalid</code>"));
        assert!(!body.contains("The answer is"));
    }

    #[tokio::test]
    async fn wrong_answer_gets_no_explainer() {
        let (status, body) = post_answer("phone-home", "stats.pagepilot-assets.invalid").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("\"correct\":false"));
        assert!(!body.contains("explainer_html"));
    }

    #[tokio::test]
    async fn right_answer_returns_explainer() {
        let (_, body) = post_answer("phone-home", "  Sync.PagePilot-CDN.invalid ").await;
        assert!(body.contains("\"correct\":true"));
        assert!(body.contains("explainer_html"));
        let (status, _) = post_answer("nope", "x").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn raw_files_are_limited_to_the_pack() {
        let (status, body) = get_body("/p/peanuts/files/cookies.txt").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("session_token"));
        for bad in [
            "/p/peanuts/files/../answer",
            "/p/peanuts/files/%2e%2e/answer",
            "/p/peanuts/files/missing.txt",
        ] {
            assert_eq!(get_body(bad).await.0, StatusCode::NOT_FOUND, "{bad}");
        }
    }

    #[tokio::test]
    async fn security_headers_present() {
        let res = app()
            .oneshot(Request::get("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert!(res.headers().contains_key(header::CONTENT_SECURITY_POLICY));
        assert_eq!(res.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    }
}
