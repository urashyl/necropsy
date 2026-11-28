// SPDX-License-Identifier: AGPL-3.0-or-later
//! Proofs, moderators and the audit log (docs/moderation.md).

use super::*;
use rusqlite::params;

/// c2-gate is a live daily dated 2026-11-27.
const DAILY: &str = "c2-gate";

/// A live site on c2-gate's day whose boards ask the top `n` for proofs.
async fn site_top(n: usize) -> Site {
    let Site { app, state, mail } = live_site_on("2026-11-27").await;
    drop(app);
    let mut state = Arc::try_unwrap(state).ok().expect("state is shared");
    state.top_n = n;
    let state = Arc::new(state);
    Site {
        app: router(state.clone()),
        state,
        mail,
    }
}

async fn uid(s: &Site, name: &str) -> i64 {
    let name = name.to_owned();
    s.state
        .db
        .run(move |c| {
            c.query_row("SELECT id FROM users WHERE username = ?1", [name], |r| {
                r.get(0)
            })
        })
        .await
        .unwrap()
}

/// A ranked solve by a new player `ago` seconds after starting. Returns the
/// player's cookie and the answer's JSON.
async fn ranked_solve(s: &Site, name: &str, ago: i64) -> (String, String) {
    let (cookie, label) = spin_up(s, name, DAILY).await;
    let id = uid(s, name).await;
    s.state
        .db
        .run(move |c| {
            c.execute_batch("DROP TRIGGER IF EXISTS ranked_attempts_final")?;
            c.execute(
                "UPDATE ranked_attempts SET started_at = started_at - ?2 WHERE user_id = ?1",
                params![id, ago],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let flag = pebble_flag(s, &label).await;
    let r = s
        .post_json(
            &format!("/api/p/{DAILY}/answer"),
            &format!(r#"{{"answer":"{flag}"}}"#),
            Some(&cookie),
        )
        .await;
    assert!(r.body.contains(r#""attempt":"ranked""#), "{}", r.body);
    (cookie, r.body)
}

async fn proof_status(s: &Site, name: &str) -> Option<String> {
    let id = uid(s, name).await;
    s.state
        .db
        .proof_of(id, DAILY.into())
        .await
        .unwrap()
        .map(|p| p.status)
}

async fn audit_actions(s: &Site) -> Vec<String> {
    s.state
        .db
        .audit_recent(100)
        .await
        .unwrap()
        .into_iter()
        .map(|e| format!("{} {} {}", e.actor, e.action, e.target.unwrap_or_default()))
        .collect()
}

/// Moves a started proof's deadline into the past, or an unstarted one's offer.
async fn age_proof(s: &Site, name: &str, secs: i64) {
    let id = uid(s, name).await;
    s.state
        .db
        .run(move |c| {
            c.execute(
                "UPDATE proofs SET offered_at = offered_at - ?2,
                     opened_at = opened_at - ?2, deadline = deadline - ?2 WHERE user_id = ?1",
                params![id, secs],
            )
        })
        .await
        .unwrap();
}

async fn moderator(s: &Site, name: &str) -> String {
    s.register_verified(name, &format!("{name}@example.org"))
        .await;
    let admin = s.admin_cookie().await;
    let r = s
        .post(
            "/admin/moderators",
            &[("username", name), ("action", "add")],
            Some(&admin),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    s.login_cookie(name).await
}

const TEXT: &str = "Followed the check-in from the sample, then sent the tasking request with the implant's user agent.";

#[tokio::test]
async fn a_top_solve_writes_a_proof_in_time_and_a_moderator_approves_it() {
    let s = site_top(1).await;
    let (alice, body) = ranked_solve(&s, "alice", 120).await;
    assert!(body.contains(r#""proof_url":"/proof/c2-gate""#), "{body}");
    assert_eq!(proof_status(&s, "alice").await.as_deref(), Some("open"));
    let board = s.get("/leaderboard/ranked", None).await.body;
    assert!(board.contains("Proof due"), "{board}");

    // Before Start there is no box, and sending is refused.
    let page = s.get("/proof/c2-gate", Some(&alice)).await.body;
    assert!(page.contains("AI-generated explanations are not accepted."));
    assert!(!page.contains("proof-text"));
    let early = s
        .post("/proof/c2-gate", &[("text", TEXT)], Some(&alice))
        .await;
    assert_eq!(early.status, StatusCode::CONFLICT);

    assert_eq!(
        s.post("/proof/c2-gate/start", &[], Some(&alice))
            .await
            .status,
        StatusCode::SEE_OTHER
    );
    let page = s.get("/proof/c2-gate", Some(&alice)).await.body;
    assert!(page.contains(r#"id="proof-text""#), "{page}");
    assert!(
        page.contains(r#"data-left="90""#) || page.contains(r#"data-left="89""#),
        "{page}"
    );
    // Too short.
    assert_eq!(
        s.post("/proof/c2-gate", &[("text", "it was easy")], Some(&alice))
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        s.post("/proof/c2-gate", &[("text", TEXT)], Some(&alice))
            .await
            .status,
        StatusCode::SEE_OTHER
    );
    assert_eq!(proof_status(&s, "alice").await.as_deref(), Some("pending"));
    // Sent once only.
    assert_eq!(
        s.post("/proof/c2-gate", &[("text", TEXT)], Some(&alice))
            .await
            .status,
        StatusCode::CONFLICT
    );

    let mo = moderator(&s, "moda").await;
    let queue = s.get("/mod/proofs", Some(&mo)).await.body;
    assert!(
        queue.contains("alice") && queue.contains("tasking request"),
        "{queue}"
    );
    let pid = s.state.db.proofs_pending().await.unwrap()[0].id;
    assert_eq!(
        s.post(&format!("/mod/proofs/{pid}/approve"), &[], Some(&mo))
            .await
            .status,
        StatusCode::SEE_OTHER
    );
    assert_eq!(proof_status(&s, "alice").await.as_deref(), Some("approved"));
    assert!(s
        .get("/leaderboard/ranked", None)
        .await
        .body
        .contains("Verified"));
    // Reviewed once only.
    assert_eq!(
        s.post(
            &format!("/mod/proofs/{pid}/reject"),
            &[("reason", "x")],
            Some(&mo)
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    let log = audit_actions(&s).await;
    assert!(
        log.contains(&"moda proof approved alice".to_owned()),
        "{log:?}"
    );
}

#[tokio::test]
async fn a_late_or_missing_proof_is_forfeited_and_the_next_player_is_asked() {
    let s = site_top(1).await;
    let (alice, _) = ranked_solve(&s, "alice", 60).await;
    let (bob, body) = ranked_solve(&s, "bob", 300).await;
    assert!(
        !body.contains("proof_url"),
        "second place is not asked with a top of 1"
    );
    assert_eq!(proof_status(&s, "bob").await, None);

    // Alice starts, and sends after the deadline and its grace.
    s.post("/proof/c2-gate/start", &[], Some(&alice)).await;
    age_proof(
        &s,
        "alice",
        crate::moderation::PROOF_SECONDS + crate::moderation::PROOF_GRACE + 1,
    )
    .await;
    let mails_before = s.mail.lock().unwrap().len();
    assert_eq!(
        s.post("/proof/c2-gate", &[("text", TEXT)], Some(&alice))
            .await
            .status,
        StatusCode::CONFLICT
    );
    // Reading the board settles it: alice is off, bob moves up and is asked.
    let board = s.get("/leaderboard/ranked", None).await.body;
    assert!(
        !board.contains(">alice<") && board.contains(">bob<"),
        "{board}"
    );
    assert_eq!(
        proof_status(&s, "alice").await.as_deref(),
        Some("forfeited")
    );
    assert_eq!(proof_status(&s, "bob").await.as_deref(), Some("open"));
    let emailed = s.mail.lock().unwrap()[mails_before..]
        .iter()
        .any(|m| m.to == "bob@example.org" && m.body.contains("/proof/c2-gate"));
    assert!(emailed, "bob is emailed");
    // Bob sees the notice on his next visit.
    assert!(s
        .get("/account", Some(&bob))
        .await
        .body
        .contains("needs a proof"));

    // Bob never starts: a day later his is forfeited too.
    age_proof(&s, "bob", crate::moderation::PROOF_OPEN_WINDOW + 1).await;
    assert!(!s
        .get("/leaderboard/ranked", None)
        .await
        .body
        .contains(">bob<"));
    assert_eq!(proof_status(&s, "bob").await.as_deref(), Some("forfeited"));
    let log = audit_actions(&s).await;
    assert!(
        log.contains(&"the site proof forfeited alice".to_owned()),
        "{log:?}"
    );
    assert!(
        log.contains(&"the site proof forfeited bob".to_owned()),
        "{log:?}"
    );
}

#[tokio::test]
async fn a_rejected_proof_takes_the_entry_off_and_promotes_the_next() {
    let s = site_top(1).await;
    let (alice, _) = ranked_solve(&s, "alice", 60).await;
    ranked_solve(&s, "bob", 300).await;
    s.post("/proof/c2-gate/start", &[], Some(&alice)).await;
    s.post("/proof/c2-gate", &[("text", TEXT)], Some(&alice))
        .await;
    let mo = moderator(&s, "moda").await;
    let pid = s.state.db.proofs_pending().await.unwrap()[0].id;
    // A rejection needs a reason.
    assert_eq!(
        s.post(
            &format!("/mod/proofs/{pid}/reject"),
            &[("reason", "")],
            Some(&mo)
        )
        .await
        .status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        s.post(
            &format!("/mod/proofs/{pid}/reject"),
            &[("reason", "reads as generated")],
            Some(&mo)
        )
        .await
        .status,
        StatusCode::SEE_OTHER
    );
    assert_eq!(proof_status(&s, "bob").await.as_deref(), Some("open"));
    let board = s.get("/leaderboard/ranked", None).await.body;
    assert!(!board.contains(">alice<"), "{board}");
    let as_mod = s.get("/leaderboard/ranked", Some(&mo)).await.body;
    assert!(
        as_mod.contains(">alice<") && as_mod.contains("reads as generated"),
        "{as_mod}"
    );
    let page = s.get("/proof/c2-gate", Some(&alice)).await.body;
    assert!(page.contains("reads as generated"), "{page}");
    let log = audit_actions(&s).await;
    assert!(
        log.contains(&"moda proof rejected alice".to_owned()),
        "{log:?}"
    );
}

#[tokio::test]
async fn removing_an_entry_is_audited_and_promotes_the_next() {
    let s = site_top(1).await;
    ranked_solve(&s, "alice", 60).await;
    ranked_solve(&s, "bob", 300).await;
    let mo = moderator(&s, "moda").await;
    let alice = uid(&s, "alice").await;
    assert_eq!(
        s.post(
            &format!("/mod/board/{DAILY}/{alice}/remove"),
            &[("reason", "shared flag")],
            Some(&mo)
        )
        .await
        .status,
        StatusCode::SEE_OTHER
    );
    assert_eq!(proof_status(&s, "bob").await.as_deref(), Some("open"));
    assert_eq!(
        s.post(
            &format!("/mod/board/{DAILY}/{alice}/reinstate"),
            &[],
            Some(&mo)
        )
        .await
        .status,
        StatusCode::SEE_OTHER
    );
    let log = audit_actions(&s).await;
    assert!(
        log.contains(&"moda entry removed alice".to_owned()),
        "{log:?}"
    );
    assert!(
        log.contains(&"moda entry reinstated alice".to_owned()),
        "{log:?}"
    );
    // The audit row holds the reason.
    let entries = s.state.db.audit_recent(10).await.unwrap();
    assert!(entries
        .iter()
        .any(|e| e.reason.as_deref() == Some("shared flag")));
}

#[tokio::test]
async fn a_moderator_reaches_moderation_and_nothing_else_an_admin_has() {
    let s = site_top(10).await;
    let mo = moderator(&s, "moda").await;
    s.register_verified("pat", "pat@example.org").await;
    let pat = s.login_cookie("pat").await;
    let some_user = uid(&s, "pat").await;

    // What a moderator may do.
    assert_eq!(s.get("/mod/proofs", Some(&mo)).await.status, StatusCode::OK);
    // Every admin route stays closed to them, as to a player.
    let admin_gets = [
        "/admin".to_owned(),
        "/admin/moderators".to_owned(),
        "/admin/audit".to_owned(),
        format!("/admin/users/{some_user}/delete"),
    ];
    let admin_posts = [
        "/admin/moderators".to_owned(),
        "/admin/test-email".to_owned(),
        format!("/admin/users/{some_user}/delete"),
        format!("/admin/users/{some_user}/disable"),
        format!("/admin/users/{some_user}/verify"),
        format!("/admin/live/{DAILY}/start"),
        format!("/admin/live/{DAILY}/stop"),
    ];
    for who in [&mo, &pat] {
        for path in &admin_gets {
            assert_eq!(
                s.get(path, Some(who)).await.status,
                StatusCode::NOT_FOUND,
                "GET {path}"
            );
        }
        for path in &admin_posts {
            assert_eq!(
                s.post(path, &[("username", "pat"), ("action", "add")], Some(who))
                    .await
                    .status,
                StatusCode::NOT_FOUND,
                "POST {path}"
            );
        }
    }
    // A moderator cannot make another one.
    assert!(
        !s.state
            .db
            .user_by_id(some_user)
            .await
            .unwrap()
            .unwrap()
            .moderator
    );

    // Moderation is closed to players, and visitors are sent to log in.
    for path in [
        "/mod/proofs/1/approve".to_owned(),
        format!("/mod/board/{DAILY}/{some_user}/remove"),
    ] {
        assert_eq!(
            s.post(&path, &[("reason", "x")], Some(&pat)).await.status,
            StatusCode::NOT_FOUND,
            "{path}"
        );
        let anon = s.post(&path, &[("reason", "x")], None).await;
        assert_eq!(
            anon.headers
                .get(header::LOCATION)
                .and_then(|l| l.to_str().ok()),
            Some("/login"),
            "{path}"
        );
    }
    assert_eq!(
        s.get("/mod/proofs", Some(&pat)).await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn admins_name_moderators_and_every_change_is_audited() {
    let s = site_top(10).await;
    s.register_verified("quinn", "quinn@example.org").await;
    let admin = s.admin_cookie().await;
    let add = s
        .post(
            "/admin/moderators",
            &[("username", "quinn"), ("action", "add")],
            Some(&admin),
        )
        .await;
    assert!(add.body.contains("quinn is a moderator."), "{}", add.body);
    let quinn = s.login_cookie("quinn").await;
    assert_eq!(
        s.get("/mod/proofs", Some(&quinn)).await.status,
        StatusCode::OK
    );
    assert!(s
        .get("/", Some(&quinn))
        .await
        .body
        .contains(r#"href="/mod/proofs""#));

    let remove = s
        .post(
            "/admin/moderators",
            &[("username", "quinn"), ("action", "remove")],
            Some(&admin),
        )
        .await;
    assert!(
        remove.body.contains("no longer a moderator"),
        "{}",
        remove.body
    );
    assert_eq!(
        s.get("/mod/proofs", Some(&quinn)).await.status,
        StatusCode::NOT_FOUND
    );

    let unknown = s
        .post(
            "/admin/moderators",
            &[("username", "nobody"), ("action", "add")],
            Some(&admin),
        )
        .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);

    let audit = s.get("/admin/audit", Some(&admin)).await.body;
    assert!(
        audit.contains("moderator added") && audit.contains("moderator removed"),
        "{audit}"
    );
}

#[tokio::test]
async fn a_player_with_no_proof_asked_has_no_proof_page() {
    let s = site_top(1).await;
    s.register_verified("rae", "rae@example.org").await;
    let rae = s.login_cookie("rae").await;
    assert_eq!(
        s.get("/proof/c2-gate", Some(&rae)).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        s.post("/proof/c2-gate/start", &[], Some(&rae)).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        s.post("/proof/c2-gate", &[("text", TEXT)], Some(&rae))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}
