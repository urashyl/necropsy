// SPDX-License-Identifier: AGPL-3.0-or-later
//! Lab traffic. A site server forwards a player's request here with the lab's id
//! in `X-Necropsy-Instance`; it is passed on to that lab's unix socket and the
//! answer comes back. Labs have no network of their own, so this is their only
//! door, and a tenant can only reach its own labs.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, HeaderName, StatusCode};
use axum::response::{IntoResponse, Response};
use http_body_util::Full;

use crate::App;

/// Largest request body passed to a lab, and largest answer passed back.
const MAX_REQUEST: usize = 1024 * 1024;
const MAX_RESPONSE: usize = 8 * 1024 * 1024;

/// Headers that belong to one connection and are never passed along. The
/// `x-necropsy-*` ones are ours: the site sets them, and a lab never sees them.
pub fn dropped(name: &HeaderName) -> bool {
    let n = name.as_str();
    n.starts_with("x-necropsy-")
        || matches!(
            n,
            "connection"
                | "keep-alive"
                | "proxy-authenticate"
                | "proxy-authorization"
                | "te"
                | "trailer"
                | "transfer-encoding"
                | "upgrade"
                | "content-length"
        )
}

pub async fn handle(app: &Arc<App>, tenant: &str, id: &str, req: Request) -> Response {
    if !crate::valid_id(id) {
        return (StatusCode::BAD_REQUEST, "bad instance id").into_response();
    }
    if !app.lock().contains_key(&(tenant.to_owned(), id.to_owned())) {
        return (StatusCode::NOT_FOUND, "no such instance").into_response();
    }
    let (parts, body) = req.into_parts();
    let body = match axum::body::to_bytes(body, MAX_REQUEST).await {
        Ok(b) => b,
        Err(_) => return (StatusCode::PAYLOAD_TOO_LARGE, "request body too large").into_response(),
    };

    let mut b = hyper::Request::builder().method(parts.method).uri(
        parts
            .uri
            .path_and_query()
            .map(|p| p.as_str())
            .unwrap_or("/"),
    );
    for (k, v) in &parts.headers {
        if !dropped(k) {
            b = b.header(k, v);
        }
    }
    let out = match b
        .header(header::CONNECTION, "close")
        .header(header::CONTENT_LENGTH, body.len())
        .body(Full::new(body))
    {
        Ok(r) => r,
        Err(_) => return (StatusCode::BAD_REQUEST, "bad request").into_response(),
    };

    let socket = app.cfg.range_dir.join(tenant).join(id).join("app.sock");
    let res = match crate::uds::send(&socket, out, MAX_RESPONSE).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("lab {tenant}/{id} did not answer: {e}");
            return (StatusCode::BAD_GATEWAY, "the lab is not answering").into_response();
        }
    };
    let (rp, rbody) = res.into_parts();
    let mut r = Response::builder().status(rp.status);
    for (k, v) in &rp.headers {
        if !dropped(k) {
            r = r.header(k, v);
        }
    }
    r.body(Body::from(rbody))
        .unwrap_or_else(|_| (StatusCode::BAD_GATEWAY, "bad answer from the lab").into_response())
}
