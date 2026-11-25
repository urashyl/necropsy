// SPDX-License-Identifier: AGPL-3.0-or-later
//! A minimal HTTP/1.1 client over a unix socket, for talking to the spawner
//! and to the container behind a live instance. One connection per request,
//! with a timeout and a cap on the size of the answer.

use std::path::Path;
use std::time::Duration;

use axum::body::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use tokio::net::UnixStream;

/// Longest a request may take, connecting and reading the answer included.
pub const TIMEOUT: Duration = Duration::from_secs(30);

/// Sends `req` to the server listening on `socket` and returns its answer
/// with the body read in full, up to `max_body` bytes.
pub async fn send(
    socket: &Path,
    req: Request<Full<Bytes>>,
    max_body: usize,
) -> Result<Response<Bytes>, String> {
    let work = async {
        let stream = UnixStream::connect(socket)
            .await
            .map_err(|e| format!("connect: {e}"))?;
        let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(|e| format!("handshake: {e}"))?;
        tokio::spawn(async move {
            let _ = conn.await;
        });
        let res = sender
            .send_request(req)
            .await
            .map_err(|e| format!("request: {e}"))?;
        let (parts, body) = res.into_parts();
        let bytes = Limited::new(body, max_body)
            .collect()
            .await
            .map_err(|e| format!("body: {e}"))?
            .to_bytes();
        Ok(Response::from_parts(parts, bytes))
    };
    tokio::time::timeout(TIMEOUT, work)
        .await
        .map_err(|_| "timed out".to_owned())?
}
