// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Learn track: module pages, tool paths, checkpoints and progress.

use super::*;

const PCAP_ANSWERS: [(&str, &str); 4] = [
    ("a0", "http.request"),
    ("a1", "tcp.stream eq 3"),
    ("a2", "Host"),
    ("a3", "192.0.2.77"),
];

fn cookie_of(r: &Reply, name: &str) -> Option<String> {
    r.headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with(&format!("{name}=")))
        .map(|v| v.split(';').next().unwrap().to_owned())
}

#[tokio::test]
async fn the_index_lists_every_module_by_tier_and_the_practice_puzzles() {
    let s = site().await;
    let page = s.get("/learn", None).await.body;
    let mut last = 0;
    for (i, m) in s.state.learn.iter().enumerate() {
        let at = page
            .find(&format!("href=\"/learn/{}\"", m.module.meta.id))
            .unwrap_or_else(|| panic!("{} missing", m.module.meta.id));
        assert!(at > last || i == 0, "{} out of order", m.module.meta.id);
        last = at;
    }
    for t in 1..=5 {
        assert!(page.contains(&format!("data-tier=\"{t}\"")), "tier {t}");
    }
    assert!(page.contains("In the browser") && page.contains("In a terminal"));
    assert!(page.contains("All practice puzzles"));
}

#[tokio::test]
async fn module_pages_have_all_four_parts() {
    let s = site().await;
    for m in &s.state.learn {
        let id = &m.module.meta.id;
        let r = s.get(&format!("/learn/{id}"), None).await;
        assert_eq!(r.status, StatusCode::OK, "{id}");
        for part in [
            "The idea",
            "Walkthrough: in the browser",
            "Practice",
            "Checkpoint",
        ] {
            assert!(r.body.contains(part), "{id}: {part}");
        }
    }
    assert_eq!(
        s.get("/learn/nope", None).await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn the_path_switches_the_walkthrough_and_keeps_progress() {
    let s = site().await;
    let page = s.get("/learn/reading-a-pcap", None).await.body;
    assert!(page.contains("Walkthrough: in the browser"));
    assert!(
        page.contains("Open the practice puzzle, then **Open the workspace**")
            || page.contains("Open the workspace")
    );
    // Signed out: the choice is a cookie.
    let r = s
        .post(
            "/learn/path",
            &[("path", "terminal"), ("back", "/learn/reading-a-pcap")],
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(r.headers[header::LOCATION], "/learn/reading-a-pcap");
    let c = cookie_of(&r, "necropsy_path").unwrap();
    assert_eq!(c, "necropsy_path=terminal");
    let page = s.get("/learn/reading-a-pcap", Some(&c)).await.body;
    assert!(page.contains("Walkthrough: in a terminal"));
    assert!(page.contains("tshark -r capture.pcap"));
    assert!(page.contains("Switch to the browser path"));
    // ?path= shows the other one without changing the choice.
    let once = s
        .get("/learn/reading-a-pcap?path=browser", Some(&c))
        .await
        .body;
    assert!(once.contains("Walkthrough: in the browser"));

    // Signed in: it is kept on the account, and passing a checkpoint is not lost by switching.
    s.register_verified("learner", "learner@example.org").await;
    let cookie = s.login_cookie("learner").await;
    let r = s
        .post(
            "/learn/reading-a-pcap/checkpoint",
            &PCAP_ANSWERS,
            Some(&cookie),
        )
        .await;
    assert!(!r.body.contains("passed-note hidden"), "{}", r.body);
    s.post(
        "/learn/path",
        &[("path", "terminal"), ("back", "/learn")],
        Some(&cookie),
    )
    .await;
    s.post(
        "/learn/path",
        &[("path", "browser"), ("back", "/learn")],
        Some(&cookie),
    )
    .await;
    let me = s.get("/api/me/learn", Some(&cookie)).await.body;
    assert!(me.contains("\"path\":\"browser\""), "{me}");
    assert!(me.contains("\"passed\":[\"reading-a-pcap\"]"), "{me}");
    // The account's choice wins over a stale cookie.
    let both = format!("{cookie}; necropsy_path=terminal");
    let page = s.get("/learn/reading-a-pcap", Some(&both)).await.body;
    assert!(page.contains("Walkthrough: in the browser"));
}

#[tokio::test]
async fn the_path_form_refuses_bad_values_and_foreign_redirects() {
    let s = site().await;
    let r = s
        .post("/learn/path", &[("path", "gui"), ("back", "/learn")], None)
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    for back in [
        "https://evil.test/",
        "//evil.test",
        "/learn/../admin",
        "/admin",
        "/learn/x?y",
    ] {
        let r = s
            .post("/learn/path", &[("path", "browser"), ("back", back)], None)
            .await;
        assert_eq!(r.headers[header::LOCATION], "/learn", "{back}");
    }
}

#[tokio::test]
async fn checkpoints_grade_each_answer_and_record_only_a_full_pass() {
    let s = site().await;
    s.register_verified("checker", "checker@example.org").await;
    let cookie = s.login_cookie("checker").await;
    let wrong = [
        ("a0", "http"),
        ("a1", "tcp.stream == 3"),
        ("a2", "Host"),
        ("a3", "192.0.2.77"),
    ];
    let r = s
        .post("/learn/reading-a-pcap/checkpoint", &wrong, Some(&cookie))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body.matches("Right.").count(), 3, "{}", r.body);
    assert!(r.body.contains("Not it."));
    assert!(
        r.body.contains("HTTP fields start with http"),
        "the question's hint"
    );
    assert!(r.body.contains("passed-note hidden"), "not passed");
    // The answers stay in the form.
    assert!(r.body.contains("value=\"192.0.2.77\""));
    assert!(s
        .get("/api/me/learn", Some(&cookie))
        .await
        .body
        .contains("\"passed\":[]"));

    let r = s
        .post(
            "/learn/reading-a-pcap/checkpoint",
            &PCAP_ANSWERS,
            Some(&cookie),
        )
        .await;
    assert_eq!(r.body.matches("Right.").count(), 4);
    let idx = s.get("/learn", Some(&cookie)).await.body;
    assert!(idx.contains("data-passed=\"reading-a-pcap\""), "{idx}");

    // Signed out: graded, nothing stored, and the page says so.
    let r = s
        .post("/learn/reading-a-pcap/checkpoint", &PCAP_ANSWERS, None)
        .await;
    assert!(r.body.contains("Kept in this browser"));
    assert_eq!(
        s.post("/learn/nope/checkpoint", &PCAP_ANSWERS, None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn curl_questions_check_the_request_not_the_spelling() {
    let s = site().await;
    let good = [
        (
            "a0",
            "curl 'https://c2.example.test/status?id=7' -H 'x-key: abc'",
        ),
        (
            "a1",
            "curl -H 'User-Agent: Pebble/2.4' -d 'id=7&st=idle' https://c2.example.test/gate",
        ),
        ("a2", "--include"),
        ("a3", "the method"),
    ];
    let r = s.post("/learn/curl-by-hand/checkpoint", &good, None).await;
    assert_eq!(r.body.matches("Right.").count(), 4, "{}", r.body);
    let bad = [
        ("a0", "curl -A abc https://c2.example.test/status?id=7"),
        (
            "a1",
            "curl -X POST -H 'User-Agent: Pebble/2.4' https://c2.example.test/gate",
        ),
        ("a2", "-v"),
        ("a3", "the path"),
    ];
    let r = s.post("/learn/curl-by-hand/checkpoint", &bad, None).await;
    assert_eq!(r.body.matches("Not it.").count(), 4, "{}", r.body);
}

#[tokio::test]
async fn the_json_checkpoint_uploads_a_signed_out_pass() {
    let s = site().await;
    let body = r#"{"answers":["http.request","tcp.stream == 3","Host","192.0.2.77"]}"#;
    let r = s
        .post_json("/api/learn/reading-a-pcap/checkpoint", body, None)
        .await;
    assert!(r.body.contains("\"passed\":true"), "{}", r.body);
    s.register_verified("uploader", "uploader@example.org")
        .await;
    let cookie = s.login_cookie("uploader").await;
    let r = s
        .post_json(
            "/api/learn/reading-a-pcap/checkpoint",
            r#"{"answers":["x"]}"#,
            Some(&cookie),
        )
        .await;
    assert!(r.body.contains("\"passed\":false") && r.body.contains("[false,false,false,false]"));
    s.post_json("/api/learn/reading-a-pcap/checkpoint", body, Some(&cookie))
        .await;
    assert!(s
        .get("/api/me/learn", Some(&cookie))
        .await
        .body
        .contains("\"passed\":[\"reading-a-pcap\"]"));
    let anon = s.get("/api/me/learn", None).await.body;
    assert!(anon.contains("\"user\":null") && anon.contains("\"modules\":[\"reading-a-pcap\""));
}

#[tokio::test]
async fn practice_needing_live_puzzles_says_so() {
    let s = site().await;
    let page = s.get("/learn/curl-by-hand", None).await.body;
    assert!(page.contains("needs live puzzles or the range"), "{page}");
    let s = live_site().await;
    let page = s.get("/learn/curl-by-hand", None).await.body;
    assert!(page.contains("href=\"/p/learn-knock\""));
}

#[tokio::test]
async fn closed_dailies_become_worked_examples() {
    // dns-exfil (2026-11-30) names the encodings module.
    let s = site_on("2026-11-30").await;
    let page = s.get("/learn/encodings", None).await.body;
    assert!(
        !page.contains("/p/dns-exfil"),
        "not while it is today's puzzle"
    );
    let s = site_on("2026-12-01").await;
    let page = s.get("/learn/encodings", None).await.body;
    assert!(
        page.contains("Worked examples") && page.contains("/p/dns-exfil"),
        "{page}"
    );
    let s = site_on("2026-11-29").await;
    assert!(!s
        .get("/learn/encodings", None)
        .await
        .body
        .contains("/p/dns-exfil"));
}

#[tokio::test]
async fn deleting_the_account_deletes_learn_progress() {
    let s = site().await;
    s.register_verified("gone", "gone@example.org").await;
    let cookie = s.login_cookie("gone").await;
    s.post(
        "/learn/reading-a-pcap/checkpoint",
        &PCAP_ANSWERS,
        Some(&cookie),
    )
    .await;
    let uid = s
        .state
        .db
        .user_for_login("gone".into())
        .await
        .unwrap()
        .unwrap()
        .0
        .id;
    assert_eq!(s.state.db.learn_passed(uid).await.unwrap().len(), 1);
    s.state.db.delete_user(uid).await.unwrap();
    assert!(s.state.db.learn_passed(uid).await.unwrap().is_empty());
}
