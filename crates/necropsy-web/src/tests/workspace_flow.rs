// SPDX-License-Identifier: AGPL-3.0-or-later
//! The workspace page, the toolkit's files and the request console's WebSocket.

use super::*;

fn toolkit_dir(name: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("necropsy-wstk-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (path, body) in [
        ("wiregasm-2.0.0/wiregasm.js", "var loadWiregasm;"),
        ("pyodide-314.0.8/pyodide.mjs", "export {};"),
        ("cyberchef-10.20.0/index.html", "<!doctype html>"),
    ] {
        let p = root.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, body).unwrap();
    }
    std::fs::write(root.join("wiregasm-2.0.0/wiregasm.js.gz"), b"\x1f\x8bgz").unwrap();
    root
}

fn with_toolkit(s: Site, name: &str) -> Site {
    let Site { app, state, mail } = s;
    drop(app);
    let mut state = Arc::try_unwrap(state).ok().expect("state is shared");
    state.toolkit = crate::toolkit::Toolkit::load(&toolkit_dir(name));
    let state = Arc::new(state);
    Site {
        app: router(state.clone()),
        state,
        mail,
    }
}

#[tokio::test]
async fn workspace_page_has_the_tools_and_its_own_policy() {
    let s = with_toolkit(site().await, "page");
    let r = s.get("/w/intro-log", None).await;
    assert_eq!(r.status, StatusCode::OK);
    for want in [
        r#"data-wiregasm="/toolkit/wiregasm-2.0.0""#,
        r#"data-cyberchef="/toolkit/cyberchef-10.20.0""#,
        "id=\"tab-pcap\"",
        "id=\"tab-notebook\"",
        "/static/toolkit/workspace.js?v=",
        "answer-form",
    ] {
        assert!(r.body.contains(want), "{want}");
    }
    // Not a live puzzle: no console.
    assert!(!r.body.contains("tab-console"));
    let csp = r.headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
    assert!(csp.contains("frame-src 'self'") && csp.contains("worker-src 'self'"));
    assert!(
        !csp.contains("unsafe-eval"),
        "the page itself never allows eval"
    );
    // Other pages keep the site's policy.
    let p = s.get("/p/intro-log", None).await;
    let csp = p.headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
    assert!(!csp.contains("frame-src 'self'"));
    assert!(p.body.contains("href=\"/w/intro-log\""));
}

#[tokio::test]
async fn workspace_of_an_unreleased_daily_is_not_found() {
    // dns-exfil is dated 2026-11-30.
    let s = site_on("2026-11-29").await;
    assert_eq!(
        s.get("/w/dns-exfil", None).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(s.get("/w/nope", None).await.status, StatusCode::NOT_FOUND);
    let s = site_on("2026-11-30").await;
    assert_eq!(s.get("/w/dns-exfil", None).await.status, StatusCode::OK);
}

#[tokio::test]
async fn without_the_toolkit_the_page_still_works() {
    let s = site().await;
    let r = s.get("/w/intro-log", None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(!r.body.contains("data-wiregasm"));
    assert_eq!(
        s.get("/toolkit/wiregasm-2.0.0/wiregasm.js", None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn toolkit_files_are_served_from_the_list_only() {
    let s = with_toolkit(site().await, "files");
    let r = s.get("/toolkit/wiregasm-2.0.0/wiregasm.js", None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body, "var loadWiregasm;");
    assert!(r.headers[header::CACHE_CONTROL]
        .to_str()
        .unwrap()
        .contains("immutable"));
    assert!(r.headers[header::CONTENT_TYPE]
        .to_str()
        .unwrap()
        .starts_with("text/javascript"));
    assert_eq!(
        r.headers[header::CONTENT_SECURITY_POLICY],
        "default-src 'none'; sandbox; frame-ancestors 'none'"
    );
    // The gzip copy, to a browser that takes it.
    let gz = s
        .send(
            Request::get("/toolkit/wiregasm-2.0.0/wiregasm.js")
                .header(header::ACCEPT_ENCODING, "gzip")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(gz.headers[header::CONTENT_ENCODING], "gzip");
    for bad in [
        "/toolkit/wiregasm-2.0.0/../../etc/passwd",
        "/toolkit/wiregasm-2.0.0/%2e%2e/x",
        "/toolkit/wiregasm-2.0.0/wiregasm.js.gz",
        "/toolkit/nope",
    ] {
        assert_eq!(
            s.get(bad, None).await.status,
            StatusCode::NOT_FOUND,
            "{bad}"
        );
    }
    // CyberChef is sandboxed into an opaque origin and may be framed by the site.
    let cc = s.get("/toolkit/cyberchef-10.20.0/index.html", None).await;
    let csp = cc.headers[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap();
    assert!(csp.starts_with("sandbox allow-scripts") && !csp.contains("allow-same-origin"));
    assert!(csp.contains("frame-ancestors 'self'"));
    assert_eq!(cc.headers[header::X_FRAME_OPTIONS], "SAMEORIGIN");
}

#[tokio::test]
async fn toolkit_scripts_revalidate_and_workers_get_their_policy() {
    let s = site().await;
    let r = s.get("/static/toolkit/curl.js", None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.contains("export function parseCurl"));
    assert_eq!(r.headers[header::CACHE_CONTROL], "no-cache");
    assert!(r.headers[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap()
        .contains("default-src 'none'; script-src 'self' https://challenges"));
    let etag = r.headers[header::ETAG].to_str().unwrap().to_owned();
    let again = s
        .send(
            Request::get("/static/toolkit/curl.js")
                .header(header::IF_NONE_MATCH, &etag)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(again.status, StatusCode::NOT_MODIFIED);
    let w = s.get("/static/toolkit/pcap-worker.js", None).await;
    let csp = w.headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
    assert!(csp.contains("'unsafe-eval'") && csp.contains("'wasm-unsafe-eval'"));
    assert_eq!(
        s.get("/static/toolkit/nope.js", None).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        s.get("/static/toolkit/necropsy.py", None).await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn live_workspace_has_the_console() {
    let s = live_site().await;
    let r = s.get("/w/c2-gate", None).await;
    assert!(r.body.contains("tab-console") && r.body.contains("console-cmd"));
}

// ---- the console's WebSocket ----------------------------------------------------

fn ws_get(label: &str, cookie: Option<&str>, origin: Option<&str>) -> Request<Body> {
    let mut b = Request::get(format!("/api/live/{label}/ws"))
        .header(header::CONNECTION, "upgrade")
        .header(header::UPGRADE, "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==");
    if let Some(c) = cookie {
        b = b.header(header::COOKIE, c);
    }
    if let Some(o) = origin {
        b = b.header(header::ORIGIN, o);
    }
    b.body(Body::empty()).unwrap()
}

#[tokio::test]
async fn console_socket_refusals() {
    let s = live_site().await;
    let (cookie, label) = spin_up(&s, "wsowner", "c2-gate").await;
    // Another page, or no Origin at all.
    for origin in [
        None,
        Some("https://evil.test"),
        Some(&*format!("https://{label}.{LIVE_DOMAIN}")),
    ] {
        let r = s.send(ws_get(&label, Some(&cookie), origin)).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{origin:?}");
    }
    // Not signed in.
    let r = s.send(ws_get(&label, None, Some(ORIGIN))).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    // Someone else's instance, and one that does not exist.
    s.register_verified("wsother", "wsother@example.org").await;
    let other = s.login_cookie("wsother").await;
    let r = s.send(ws_get(&label, Some(&other), Some(ORIGIN))).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = s
        .send(ws_get("no-such-label", Some(&cookie), Some(ORIGIN)))
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    // A stopped instance.
    let stop = s
        .send(
            Request::post("/api/p/c2-gate/instance/stop")
                .header(header::COOKIE, &cookie)
                .header(header::ORIGIN, ORIGIN)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(stop.status, StatusCode::OK);
    let r = s.send(ws_get(&label, Some(&cookie), Some(ORIGIN))).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn console_without_live_puzzles_is_not_found() {
    let s = site().await;
    s.register_verified("wsnolive", "wsnolive@example.org")
        .await;
    let c = s.login_cookie("wsnolive").await;
    let r = s
        .send(ws_get("plank-orbit-cargo", Some(&c), Some(ORIGIN)))
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

/// The console's request and the same request sent to the instance's own host
/// get the same answer, forbidden headers included.
#[tokio::test]
async fn console_exchange_is_the_instance_code_path() {
    let s = live_site().await;
    let (_, label) = spin_up(&s, "wsxchg", "c2-gate").await;
    let peer = crate::wsconsole::Peer {
        label: label.clone(),
        host: format!("{label}.{LIVE_DOMAIN}"),
        client_ip: "192.0.2.1".into(),
        conn: None,
    };
    let body = "d=N2YzYTljMWU1YjJkNDA4NnxTQU5EQk9YLTAxfGFuYWx5c3R8V2luZG93cyAxMCAxOTA0NQ%3D%3D";
    let raw = format!(
        "POST /api/v2/sync HTTP/1.1\r\nHost: anything.example\r\nUser-Agent: {PEBBLE}\r\n\
         X-Client-Id: 7f3a9c1e5b2d4086\r\nContent-Type: application/x-www-form-urlencoded\r\n\
         Content-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let out = crate::wsconsole::exchange(&s.state, &peer, raw.as_bytes())
        .await
        .unwrap();
    let out = String::from_utf8(out).unwrap();
    assert!(out.starts_with("HTTP/1.1 200 OK\r\n"), "{out}");
    assert!(out.contains("\"t\":\"/api/v2/task/"), "{out}");
    // Wrong User-Agent: the decoy, as on the real host.
    let raw = "POST /api/v2/sync HTTP/1.1\r\nUser-Agent: curl/8.5.0\r\n\r\n";
    let out = crate::wsconsole::exchange(&s.state, &peer, raw.as_bytes())
        .await
        .unwrap();
    let direct = live_send(
        &s,
        &label,
        "POST",
        "/api/v2/sync",
        &[("user-agent", "curl/8.5.0")],
        "",
    )
    .await;
    let out = String::from_utf8(out).unwrap();
    assert!(out.starts_with(&format!("HTTP/1.1 {}", direct.status.as_u16())));
    assert!(out.ends_with(&direct.body));
    // A request the console cannot send says why.
    let e = crate::wsconsole::exchange(&s.state, &peer, b"GET / HTTP/1.1\r\n")
        .await
        .unwrap_err();
    assert!(e.contains("end of headers"), "{e}");
    // The instance shows the console's requests in its log, like any others.
    assert!(s
        .state
        .hub
        .recent(&label)
        .iter()
        .any(|e| e.ua.starts_with("Pebble")));
}

#[tokio::test]
async fn console_exchange_stops_with_the_instance() {
    let s = live_site().await;
    let (cookie, label) = spin_up(&s, "wsstop", "c2-gate").await;
    s.send(
        Request::post("/api/p/c2-gate/instance/stop")
            .header(header::COOKIE, &cookie)
            .header(header::ORIGIN, ORIGIN)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let peer = crate::wsconsole::Peer {
        label: label.clone(),
        host: format!("{label}.{LIVE_DOMAIN}"),
        client_ip: "192.0.2.1".into(),
        conn: None,
    };
    let e = crate::wsconsole::exchange(&s.state, &peer, b"GET / HTTP/1.1\r\n\r\n")
        .await
        .unwrap_err();
    assert!(e.contains("no longer running"), "{e}");
}

/// A real WebSocket, over TCP, from upgrade to answer.
#[tokio::test]
async fn console_socket_round_trip() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::Message;

    let s = live_site().await;
    let (cookie, label) = spin_up(&s, "wsreal", "c2-gate").await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = s.app.clone();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let mut req = format!("ws://{addr}/api/live/{label}/ws")
        .into_client_request()
        .unwrap();
    req.headers_mut().insert("origin", ORIGIN.parse().unwrap());
    req.headers_mut().insert("cookie", cookie.parse().unwrap());
    let (mut sock, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    sock.send(Message::Binary(
        format!("GET /api/v2/task/x HTTP/1.1\r\nUser-Agent: {PEBBLE}\r\n\r\n")
            .into_bytes()
            .into(),
    ))
    .await
    .unwrap();
    let reply = sock.next().await.unwrap().unwrap();
    let Message::Binary(bytes) = reply else {
        panic!("expected a binary reply, got {reply:?}");
    };
    assert!(String::from_utf8_lossy(&bytes).starts_with("HTTP/1.1 "));
    sock.send(Message::Text("not http".into())).await.unwrap();
    let reply = sock.next().await.unwrap().unwrap();
    let Message::Text(t) = reply else {
        panic!("expected an error, got {reply:?}");
    };
    assert!(t.as_str().contains("\"error\""), "{t}");
}

/// A binary file on the puzzle page opens in the workspace, not only as a
/// download; the workspace has a viewer for text files.
#[tokio::test]
async fn binary_files_open_in_the_workspace() {
    let s = site().await;
    let p = s.get("/p/learn-first-capture", None).await;
    assert_eq!(p.status, StatusCode::OK);
    assert!(
        p.body
            .contains(r#"href="/w/learn-first-capture#file=capture.pcap""#),
        "no workspace link for the capture"
    );
    assert!(p
        .body
        .contains(r#"href="/p/learn-first-capture/files/capture.pcap" download"#));
    let w = s.get("/w/learn-first-capture", None).await;
    for want in ["id=\"file-view\"", "id=\"file-view-body\""] {
        assert!(w.body.contains(want), "{want}");
    }
}
