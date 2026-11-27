// SPDX-License-Identifier: AGPL-3.0-or-later
//! Generated packs: per-player files and answers, signed in and out, and keyed
//! live flags.

use super::*;

/// kerberoast is a generated daily dated 2026-12-04.
const PACK: &str = "kerberoast";
const DAY: &str = "2026-12-04";

/// The answer the server holds for whoever sends these headers.
async fn answer_for(s: &Site, cookie: Option<&str>) -> String {
    use axum::extract::FromRequestParts;
    let mut b = Request::get("/");
    if let Some(c) = cookie {
        b = b.header(header::COOKIE, c);
    }
    let (mut parts, _) = b.body(()).unwrap().into_parts();
    let auth::MaybeUser(viewer) = auth::MaybeUser::from_request_parts(&mut parts, &s.state)
        .await
        .unwrap();
    let pack = &s.state.get(PACK).unwrap().pack;
    let (seeds, _) =
        crate::generated::seeds_for(&s.state, viewer.as_ref(), &parts.headers, pack, false).await;
    let g = s
        .state
        .generator
        .output(&s.state, pack, &seeds[0])
        .await
        .unwrap();
    g.output.answer.clone()
}

async fn csv(s: &Site, cookie: Option<&str>) -> Reply {
    s.get(&format!("/p/{PACK}/files/4769.csv"), cookie).await
}

async fn post_answer(s: &Site, cookie: Option<&str>, answer: &str) -> Reply {
    let mut b = Request::post(format!("/api/p/{PACK}/answer"))
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, ORIGIN);
    if let Some(c) = cookie {
        b = b.header(header::COOKIE, c);
    }
    s.send(
        b.body(Body::from(format!(r#"{{"answer":"{answer}"}}"#)))
            .unwrap(),
    )
    .await
}

fn anon_cookie(r: &Reply) -> String {
    r.headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("necropsy_anon="))
        .map(|v| v.split(';').next().unwrap().to_owned())
        .expect("an anon cookie")
}

#[tokio::test]
async fn each_player_gets_their_own_files_and_answer() {
    let s = site_on(DAY).await;
    s.register_verified("ann", "ann@example.org").await;
    s.register_verified("bob", "bob@example.org").await;
    let ann = s.login_cookie("ann").await;
    let bob = s.login_cookie("bob").await;
    // Opening the page gives each a seed and their own files.
    let page = s.get(&format!("/p/{PACK}"), Some(&ann)).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("These files are yours"));
    s.get(&format!("/p/{PACK}"), Some(&bob)).await;
    let (a_csv, b_csv) = (csv(&s, Some(&ann)).await, csv(&s, Some(&bob)).await);
    assert_eq!(a_csv.status, StatusCode::OK);
    assert_ne!(a_csv.body, b_csv.body);
    assert_eq!(
        csv(&s, Some(&ann)).await.body,
        a_csv.body,
        "the same files every time"
    );
    assert!(a_csv.headers[header::CACHE_CONTROL]
        .to_str()
        .unwrap()
        .contains("no-store"));

    let ann_answer = answer_for(&s, Some(&ann)).await;
    let bob_answer = answer_for(&s, Some(&bob)).await;
    assert_ne!(ann_answer, bob_answer);
    assert!(a_csv.body.contains(&ann_answer));
    // Bob cannot use Ann's answer; his own works.
    assert!(post_answer(&s, Some(&bob), &ann_answer)
        .await
        .body
        .contains("\"correct\":false"));
    assert!(post_answer(&s, Some(&bob), &bob_answer)
        .await
        .body
        .contains("\"correct\":true"));
    assert!(post_answer(&s, Some(&ann), &ann_answer)
        .await
        .body
        .contains("\"correct\":true"));
    // The page shows a player's own file inline, never another's.
    let page = s.get(&format!("/p/{PACK}"), Some(&ann)).await.body;
    assert!(page.contains(&ann_answer) && !page.contains(&bob_answer));
    // The workspace lists the same files.
    let ws = s.get(&format!("/w/{PACK}"), Some(&ann)).await.body;
    assert!(ws.contains("4769.csv"));
    assert_eq!(
        s.get(&format!("/p/{PACK}/files/nope.csv"), Some(&ann))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn a_signed_out_browser_keeps_its_files_after_signing_in() {
    let s = site_on(DAY).await;
    let first = s.get(&format!("/p/{PACK}"), None).await;
    let anon = anon_cookie(&first);
    let files = csv(&s, Some(&anon)).await;
    assert!(
        files.headers.get(header::SET_COOKIE).is_none(),
        "the id is kept, not replaced"
    );
    let answer = answer_for(&s, Some(&anon)).await;
    assert!(files.body.contains(&answer));
    assert!(post_answer(&s, Some(&anon), &answer)
        .await
        .body
        .contains("\"correct\":true"));
    // Another browser, without the id, does not get it right.
    assert!(post_answer(&s, None, &answer)
        .await
        .body
        .contains("\"correct\":false"));

    // Signing in on the same browser: the account takes the browser's seed, so
    // the files stay the same and the answer uploaded from this browser counts.
    s.register_verified("carol", "carol@example.org").await;
    let session = s.login_cookie("carol").await;
    let both = format!("{session}; {anon}");
    assert_eq!(csv(&s, Some(&both)).await.body, files.body);
    assert!(post_answer(&s, Some(&both), &answer)
        .await
        .body
        .contains("\"correct\":true"));
    // And on another device later, signed in, the files are still the same.
    assert_eq!(csv(&s, Some(&session)).await.body, files.body);
}

#[tokio::test]
async fn a_forged_browser_id_is_ignored() {
    let s = site_on(DAY).await;
    let r = s
        .get(&format!("/p/{PACK}"), Some("necropsy_anon=../../x"))
        .await;
    // Not a valid id: a new one is made.
    assert!(anon_cookie(&r).len() > 20);
}

#[tokio::test]
async fn answers_are_rate_limited_leniently() {
    let s = site_on(DAY).await;
    s.register_verified("flood", "flood@example.org").await;
    let c = s.login_cookie("flood").await;
    for i in 0..10 {
        let r = post_answer(&s, Some(&c), &format!("guess{i}")).await;
        assert_eq!(r.status, StatusCode::OK, "{i}");
    }
    let r = post_answer(&s, Some(&c), "guess").await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
    // Another account is not slowed down.
    s.register_verified("calm", "calm@example.org").await;
    let other = s.login_cookie("calm").await;
    assert_eq!(
        post_answer(&s, Some(&other), "x").await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn an_unreleased_generated_daily_gives_nothing() {
    let s = site_on("2026-12-03").await;
    assert_eq!(
        s.get(&format!("/p/{PACK}"), None).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(csv(&s, None).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_server_secret_file_is_made_once_and_checked() {
    let dir = std::env::temp_dir().join(format!("necropsy-secret-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("server-secret");
    let a = crate::generated::load_secret(&path).unwrap();
    assert!(a.len() >= 32);
    assert_eq!(crate::generated::load_secret(&path).unwrap(), a, "kept");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    std::fs::write(&path, "short\n").unwrap();
    assert!(crate::generated::load_secret(&path)
        .unwrap_err()
        .contains("shorter than 32"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn live_flags_are_keyed_and_old_instances_keep_theirs() {
    let s = live_site().await;
    let (cookie, label) = spin_up(&s, "keyed", "learn-knock").await;
    let inst = s
        .state
        .db
        .live_by_label(label.clone())
        .await
        .unwrap()
        .unwrap();
    assert!(inst.keyed, "new instances are keyed");
    let scenario = s
        .state
        .get("learn-knock")
        .unwrap()
        .pack
        .live
        .clone()
        .unwrap();
    let keyed = crate::live::flag_of(&scenario, "learn-knock", &inst, s.state.generator.secret());
    let other = crate::live::flag_of(&scenario, "learn-knock", &inst, b"another secret entirely");
    assert_ne!(keyed, other, "the flag depends on the secret");
    // An instance from before keyed flags keeps the flag it was made with.
    s.state
        .db
        .run(move |c| c.execute("UPDATE live_instances SET keyed = 0", []))
        .await
        .unwrap();
    let old = s.state.db.live_by_label(label).await.unwrap().unwrap();
    let unkeyed = crate::live::flag_of(&scenario, "learn-knock", &old, s.state.generator.secret());
    assert_ne!(unkeyed, keyed);
    let r = s
        .post_json(
            "/api/p/learn-knock/answer",
            &format!(r#"{{"answer":"{unkeyed}"}}"#),
            Some(&cookie),
        )
        .await;
    assert!(r.body.contains("\"correct\":true"), "{}", r.body);
}
