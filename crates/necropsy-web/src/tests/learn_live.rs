// SPDX-License-Identifier: AGPL-3.0-or-later
//! The live Learn practice puzzles: a walkthrough of each, and the requests
//! that must not work.

use super::*;
use base64::Engine;

fn json(body: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("{e}: {body}"))
}

#[tokio::test]
async fn knock_knock() {
    let s = live_site().await;
    let (cookie, l) = spin_up(&s, "lk1", "learn-knock").await;
    let note = live_send(&s, &l, "GET", "/notes?id=3", &[], "").await.body;
    let pin = note.split("pin=").nth(1).unwrap().trim().to_owned();
    assert_eq!(
        live_send(&s, &l, "GET", "/notes", &[], "").await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        live_send(&s, &l, "GET", "/panel/login", &[], "")
            .await
            .status,
        StatusCode::METHOD_NOT_ALLOWED
    );
    let form = [("content-type", "application/x-www-form-urlencoded")];
    let body = format!("user=op&pin={pin}");
    let no_header = live_send(&s, &l, "POST", "/panel/login", &form, &body).await;
    assert_eq!(no_header.status, StatusCode::FORBIDDEN);
    let op = [
        ("content-type", "application/x-www-form-urlencoded"),
        ("x-operator", "nightjar"),
    ];
    let wrong_pin = live_send(
        &s,
        &l,
        "POST",
        "/panel/login",
        &op,
        "user=op&pin=000000000000",
    )
    .await;
    assert_eq!(wrong_pin.status, StatusCode::UNAUTHORIZED);
    let ok = live_send(&s, &l, "POST", "/panel/login", &op, &body).await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.body);
    let flag = ok.body.split("session: ").nth(1).unwrap().trim().to_owned();
    assert!(flag.starts_with("knock{"));
    assert!(submit(&s, &cookie, "learn-knock", &flag).await);
}

#[tokio::test]
async fn page_by_page() {
    let s = live_site().await;
    let (cookie, l) = spin_up(&s, "lr1", "learn-replay").await;
    let start = json(&live_send(&s, &l, "GET", "/api/export", &[], "").await.body);
    let key = start["auth"]
        .as_str()
        .unwrap()
        .split("X-Api-Key: ")
        .nth(1)
        .unwrap()
        .split(' ')
        .next()
        .unwrap()
        .to_owned();
    let first = start["first"].as_str().unwrap().to_owned();
    // Without the key, or skipping ahead, is refused.
    assert_eq!(
        live_send(&s, &l, "GET", &first, &[], "").await.status,
        StatusCode::FORBIDDEN
    );
    let mut path = first;
    let mut pages = 0;
    let mut last = serde_json::Value::Null;
    let mut second = None;
    while !path.is_empty() {
        let r = live_send(&s, &l, "GET", &path, &[("x-api-key", &key)], "").await;
        assert_eq!(r.status, StatusCode::OK, "page {pages}: {}", r.body);
        last = json(&r.body);
        pages += 1;
        path = last["next"].as_str().unwrap_or("").to_owned();
        if pages == 1 {
            second = Some(path.clone());
        }
        if pages == 2 {
            // A page asked for again is fine; the chain only needs the one before it.
            let again = live_send(
                &s,
                &l,
                "GET",
                second.as_ref().unwrap(),
                &[("x-api-key", &key)],
                "",
            )
            .await;
            assert_eq!(again.status, StatusCode::OK);
        }
    }
    assert_eq!(pages, 30);
    let flag = last["export_id"].as_str().unwrap();
    assert!(submit(&s, &cookie, "learn-replay", flag).await);

    // Another instance: page 2 before page 1 is refused.
    let (_, l2) = spin_up(&s, "lr2", "learn-replay").await;
    let start = json(&live_send(&s, &l2, "GET", "/api/export", &[], "").await.body);
    let key2 = start["auth"]
        .as_str()
        .unwrap()
        .split("X-Api-Key: ")
        .nth(1)
        .unwrap()
        .split(' ')
        .next()
        .unwrap()
        .to_owned();
    let p1 = json(
        &live_send(
            &s,
            &l2,
            "GET",
            start["first"].as_str().unwrap(),
            &[("x-api-key", &key2)],
            "",
        )
        .await
        .body,
    );
    let p2 = p1["next"].as_str().unwrap().to_owned();
    let (_, l3) = spin_up(&s, "lr3", "learn-replay").await;
    // The other instance's paths and key mean nothing here.
    assert_eq!(
        live_send(&s, &l3, "GET", &p2, &[("x-api-key", &key2)], "")
            .await
            .status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn answer_the_door() {
    let s = live_site().await;
    let (cookie, l) = spin_up(&s, "ld1", "learn-door").await;
    let ua = "Updater/5.1 (x64; channel=stable)";
    let body = base64::engine::general_purpose::STANDARD.encode("PC-01|me|Windows 10");
    // The decoys: wrong user agent, no session, and the task before checking in.
    let bad_ua = live_send(
        &s,
        &l,
        "POST",
        "/cdn/v3/collect",
        &[
            ("user-agent", "curl/8.5.0"),
            ("x-session", "0123456789abcdef"),
        ],
        &body,
    )
    .await;
    assert_eq!(bad_ua.status, StatusCode::NOT_FOUND);
    assert!(bad_ua.body.contains("cloudflare"));
    let no_session = live_send(
        &s,
        &l,
        "POST",
        "/cdn/v3/collect",
        &[("user-agent", ua)],
        &body,
    )
    .await;
    assert_eq!(no_session.status, StatusCode::NOT_FOUND);
    let not_b64 = live_send(
        &s,
        &l,
        "POST",
        "/cdn/v3/collect",
        &[("user-agent", ua), ("x-session", "0123456789abcdef")],
        "PC-01|me|Windows 10",
    )
    .await;
    assert_eq!(not_b64.status, StatusCode::NOT_FOUND);

    let r = live_send(
        &s,
        &l,
        "POST",
        "/cdn/v3/collect",
        &[("user-agent", ua), ("x-session", "0123456789abcdef")],
        &body,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let next = json(&r.body)["next"].as_str().unwrap().to_owned();
    let bytes: Vec<u8> = (0..next.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&next[i..i + 2], 16).unwrap())
        .collect();
    let path = String::from_utf8(bytes).unwrap();
    assert!(path.starts_with("/cdn/v3/j/"), "{path}");
    assert_eq!(
        live_send(&s, &l, "GET", &path, &[], "").await.status,
        StatusCode::NOT_FOUND
    );
    let t = json(
        &live_send(&s, &l, "GET", &path, &[("user-agent", ua)], "")
            .await
            .body,
    );
    let rev: String = t["v"].as_str().unwrap().chars().rev().collect();
    let flag = String::from_utf8(
        base64::engine::general_purpose::STANDARD
            .decode(rev)
            .unwrap(),
    )
    .unwrap();
    assert!(flag.starts_with("door{"), "{flag}");
    assert!(submit(&s, &cookie, "learn-door", &flag).await);

    // In a fresh instance the task path does not answer before the check-in.
    let (_, l2) = spin_up(&s, "ld2", "learn-door").await;
    assert_eq!(
        live_send(
            &s,
            &l2,
            "GET",
            "/cdn/v3/j/anything",
            &[("user-agent", ua)],
            ""
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
}
