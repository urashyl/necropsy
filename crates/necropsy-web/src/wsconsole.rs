// SPDX-License-Identifier: AGPL-3.0-or-later
//! The request console's transport: a WebSocket from the workspace page to the
//! player's own live instance. See `docs/toolkit.md`.
//!
//! A browser cannot set `User-Agent`, `Host`, `Cookie` and other forbidden
//! headers on a `fetch`, and C2 puzzles depend on them. So the console turns a
//! curl command into a raw HTTP request and sends the bytes here. They are
//! parsed into a request and given to [`live::handle`], the same code that
//! answers the instance's own subdomain, and the raw response comes back.
//!
//! It is not a proxy. The target is fixed by the label in the URL, which must
//! be an instance of the signed-in player that has not expired; the request's
//! own `Host` and any scheme or host in its target are replaced.
//!
//! Protocol: each binary (or text) message is one raw HTTP/1.1 request. The
//! reply is a binary message with the raw response, or a text message
//! `{"error": "..."}` when the request could not be sent.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};

use crate::auth::MaybeUser;
use crate::db::now;
use crate::live;
use crate::state::AppState;

/// Largest raw request: the instance's own body limit plus room for headers.
pub const MAX_REQUEST: usize = 1024 * 1024 + 64 * 1024;
/// Largest response body passed back.
const MAX_RESPONSE: usize = 8 * 1024 * 1024;
/// A console connection with no message for this long is closed.
const IDLE: Duration = Duration::from_secs(15 * 60);
const MAX_HEADERS: usize = 100;

/// `GET /api/live/{label}/ws`.
pub async fn ws(
    State(state): State<Arc<AppState>>,
    MaybeUser(viewer): MaybeUser,
    Path(label): Path<String>,
    req: Request,
) -> Response {
    // A WebSocket is not covered by the site's CSRF check, which only looks at
    // state-changing methods, and browsers send cookies with it from any page.
    // So the page must be the site's own.
    if !origin_is_site(req.headers(), &state.base_url) {
        return (StatusCode::FORBIDDEN, "Cross-site request blocked.").into_response();
    }
    let Some(user) = viewer else {
        return (StatusCode::UNAUTHORIZED, "Sign in to use the console.").into_response();
    };
    let Some(live_cfg) = state.live.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let owned = match state.db.live_by_label(label.clone()).await {
        Ok(Some(i)) => i.user_id == user.id && now() < i.expires_at,
        Ok(None) => false,
        Err(e) => {
            tracing::error!("console lookup failed: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    if !owned {
        return (StatusCode::NOT_FOUND, "No such instance.").into_response();
    }
    let host = format!("{label}.{}", live_cfg.domain);
    let client_ip = crate::client_ip_for(&state, &req);
    let conn = req.extensions().get::<ConnectInfo<SocketAddr>>().cloned();
    let upgrade = {
        use axum::extract::FromRequestParts;
        let (mut parts, _) = req.into_parts();
        WebSocketUpgrade::from_request_parts(&mut parts, &state).await
    };
    match upgrade {
        Ok(up) => up
            .max_message_size(MAX_REQUEST)
            .max_frame_size(MAX_REQUEST)
            .on_upgrade(move |socket| {
                session(
                    socket,
                    state,
                    Peer {
                        label,
                        host,
                        client_ip,
                        conn,
                    },
                )
            }),
        Err(rejection) => rejection.into_response(),
    }
}

/// Who is on the other end of a console connection.
pub struct Peer {
    pub label: String,
    /// `<label>.<live domain>`, the instance's own host.
    pub host: String,
    /// The player's address as the rate limits see it.
    pub client_ip: String,
    pub conn: Option<ConnectInfo<SocketAddr>>,
}

async fn session(mut socket: WebSocket, state: Arc<AppState>, peer: Peer) {
    loop {
        let msg = match tokio::time::timeout(IDLE, socket.recv()).await {
            Ok(Some(Ok(m))) => m,
            _ => break,
        };
        let raw: Bytes = match msg {
            Message::Binary(b) => b,
            Message::Text(t) => Bytes::copy_from_slice(t.as_str().as_bytes()),
            Message::Close(_) => break,
            Message::Ping(_) | Message::Pong(_) => continue,
        };
        let reply = match exchange(&state, &peer, &raw).await {
            Ok(bytes) => Message::Binary(bytes.into()),
            Err(e) => Message::Text(serde_json::json!({ "error": e }).to_string().into()),
        };
        if socket.send(reply).await.is_err() {
            break;
        }
    }
}

/// Sends one raw request to the instance and returns the raw response.
pub async fn exchange(state: &Arc<AppState>, peer: &Peer, raw: &[u8]) -> Result<Vec<u8>, String> {
    // The instance may have been stopped or run out since the connection opened.
    match state.db.live_by_label(peer.label.clone()).await {
        Ok(Some(i)) if now() < i.expires_at => {}
        Ok(_) => return Err("The instance is no longer running. Start it again.".into()),
        Err(e) => {
            tracing::error!("console lookup failed: {e}");
            return Err("Server error. Try again.".into());
        }
    }
    let mut req = parse_request(raw, &peer.host)?;
    // The rate limits key on the player's address, so the request cannot choose it.
    if let Some(name) = &state.client_ip_header {
        req.headers_mut().remove(name);
        if let Ok(v) = HeaderValue::from_str(&peer.client_ip) {
            req.headers_mut().insert(name.clone(), v);
        }
    }
    if let Some(c) = &peer.conn {
        req.extensions_mut().insert(*c);
    }
    let res = live::handle(state.clone(), peer.label.clone(), req).await;
    serialize_response(res).await
}

/// Whether the request comes from a page on the site itself. Unlike the form
/// check, a missing `Origin` is refused: every browser sends one on a WebSocket.
fn origin_is_site(headers: &HeaderMap, base_url: &str) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    origin
        .trim_end_matches('/')
        .eq_ignore_ascii_case(base_url.trim_end_matches('/'))
}

/// Parses a raw HTTP/1.1 request. The target is reduced to its path and query,
/// and `Host` is set to the instance's own host.
pub fn parse_request(raw: &[u8], host: &str) -> Result<Request, String> {
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut r = httparse::Request::new(&mut headers);
    let head_len = match r.parse(raw) {
        Ok(httparse::Status::Complete(n)) => n,
        Ok(httparse::Status::Partial) => {
            return Err("The request has no end of headers (a blank line).".into())
        }
        Err(httparse::Error::TooManyHeaders) => {
            return Err(format!("The request has more than {MAX_HEADERS} headers."))
        }
        Err(e) => return Err(format!("The request is not valid HTTP: {e}.")),
    };
    let method = Method::from_bytes(r.method.unwrap_or("GET").as_bytes())
        .map_err(|_| "The method is not valid.".to_owned())?;
    let target = origin_form(r.path.unwrap_or("/"));
    let uri: Uri = target
        .parse()
        .map_err(|_| format!("The path {target:?} is not valid."))?;

    let mut b = axum::http::Request::builder()
        .method(method)
        .uri(uri)
        .version(axum::http::Version::HTTP_11);
    let mut content_length: Option<usize> = None;
    for h in r.headers.iter() {
        let name = HeaderName::from_bytes(h.name.as_bytes())
            .map_err(|_| format!("The header name {:?} is not valid.", h.name))?;
        if name == header::HOST {
            continue;
        }
        if name == header::TRANSFER_ENCODING {
            return Err("Transfer-Encoding is not supported. Send the body as it is.".into());
        }
        if name == header::CONTENT_LENGTH {
            let n = std::str::from_utf8(h.value)
                .ok()
                .and_then(|v| v.trim().parse::<usize>().ok())
                .ok_or("Content-Length is not a number.")?;
            content_length = Some(n);
        }
        let value = HeaderValue::from_bytes(h.value)
            .map_err(|_| format!("The value of {} is not valid.", h.name))?;
        b = b.header(name, value);
    }
    b = b.header(
        header::HOST,
        HeaderValue::from_str(host).map_err(|_| "Bad instance host.".to_owned())?,
    );
    let rest = &raw[head_len..];
    let body = match content_length {
        Some(n) if rest.len() < n => {
            return Err(format!(
                "Content-Length says {n} bytes, but the body has {}.",
                rest.len()
            ))
        }
        Some(n) => &rest[..n],
        None => rest,
    };
    b.body(Body::from(body.to_vec()))
        .map_err(|e| format!("The request could not be built: {e}."))
}

/// `/path?q` from an origin-form or absolute-form request target.
fn origin_form(target: &str) -> String {
    let lower = target.to_ascii_lowercase();
    let rest = ["http://", "https://"]
        .iter()
        .find_map(|s| lower.starts_with(s).then(|| &target[s.len()..]));
    let t = match rest {
        Some(after) => match after.find(['/', '?']) {
            Some(i) if after[i..].starts_with('?') => format!("/{}", &after[i..]),
            Some(i) => after[i..].to_owned(),
            None => "/".to_owned(),
        },
        None => target.to_owned(),
    };
    // A fragment never goes over the wire.
    let t = t.split('#').next().unwrap_or("/").to_owned();
    if t.starts_with('/') {
        t
    } else {
        format!("/{t}")
    }
}

/// The response as it would go over the wire: status line, headers, blank
/// line, body. `Content-Length` is added when the handler did not set one, as
/// the HTTP server would.
pub async fn serialize_response(res: Response) -> Result<Vec<u8>, String> {
    let (parts, body) = res.into_parts();
    let body = axum::body::to_bytes(body, MAX_RESPONSE)
        .await
        .map_err(|_| "The response is too large to show.".to_owned())?;
    let status = parts.status;
    let mut out = format!(
        "HTTP/1.1 {} {}\r\n",
        status.as_u16(),
        status.canonical_reason().unwrap_or("")
    )
    .into_bytes();
    for (k, v) in &parts.headers {
        out.extend_from_slice(k.as_str().as_bytes());
        out.extend_from_slice(b": ");
        out.extend_from_slice(v.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    if !parts.headers.contains_key(header::CONTENT_LENGTH) {
        out.extend_from_slice(format!("content-length: {}\r\n", body.len()).as_bytes());
    }
    out.extend_from_slice(b"\r\n");
    out.extend_from_slice(&body);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOST: &str = "plank-orbit-cargo.live.example.test";

    #[test]
    fn parses_a_request_and_forces_the_host() {
        let raw = b"POST /api/v2/sync?x=1 HTTP/1.1\r\nHost: evil.example\r\nUser-Agent: Pebble/2.4\r\nCookie: sid=1\r\nContent-Length: 3\r\n\r\nd=1EXTRA";
        let r = parse_request(raw, HOST).unwrap();
        assert_eq!(r.method(), Method::POST);
        assert_eq!(r.uri().path(), "/api/v2/sync");
        assert_eq!(r.uri().query(), Some("x=1"));
        assert_eq!(r.headers()[header::HOST], HOST);
        assert_eq!(r.headers().get_all(header::HOST).iter().count(), 1);
        assert_eq!(r.headers()[header::USER_AGENT], "Pebble/2.4");
        assert_eq!(r.headers()[header::COOKIE], "sid=1");
    }

    #[test]
    fn the_target_is_reduced_to_path_and_query() {
        for (t, want) in [
            ("/a?b=1", "/a?b=1"),
            ("http://other.example/x/y?z", "/x/y?z"),
            ("HTTPS://other.example", "/"),
            ("https://other.example?q=1", "/?q=1"),
            ("/a#frag", "/a"),
            ("relative", "/relative"),
        ] {
            assert_eq!(origin_form(t), want, "{t}");
        }
    }

    #[test]
    fn bad_requests_are_refused_with_a_reason() {
        let cases: [(&[u8], &str); 5] = [
            (b"GET / HTTP/1.1\r\nHost: x\r\n", "end of headers"),
            (
                b"GET / HTTP/1.1\r\nContent-Length: 10\r\n\r\nshort",
                "Content-Length says 10",
            ),
            (
                b"GET / HTTP/1.1\r\nContent-Length: ten\r\n\r\n",
                "not a number",
            ),
            (
                b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n",
                "Transfer-Encoding",
            ),
            (b"G@T / HTTP/1.1\r\n\r\n", "not valid HTTP"),
        ];
        for (raw, want) in cases {
            let e = parse_request(raw, HOST).err().unwrap();
            assert!(e.contains(want), "{e}");
        }
    }

    #[test]
    fn the_origin_must_be_the_site() {
        let mut h = HeaderMap::new();
        assert!(!origin_is_site(&h, "https://example.test"));
        h.insert(header::ORIGIN, "https://evil.test".parse().unwrap());
        assert!(!origin_is_site(&h, "https://example.test"));
        h.insert(
            header::ORIGIN,
            "https://x.live.example.test".parse().unwrap(),
        );
        assert!(!origin_is_site(&h, "https://example.test"));
        h.insert(header::ORIGIN, "https://example.test".parse().unwrap());
        assert!(origin_is_site(&h, "https://example.test/"));
    }

    #[tokio::test]
    async fn responses_are_serialized_as_on_the_wire() {
        let res = Response::builder()
            .status(404)
            .header("x-a", "b")
            .body(Body::from("Not Found\n"))
            .unwrap();
        let out = String::from_utf8(serialize_response(res).await.unwrap()).unwrap();
        assert_eq!(
            out,
            "HTTP/1.1 404 Not Found\r\nx-a: b\r\ncontent-length: 10\r\n\r\nNot Found\n"
        );
    }
}
