// SPDX-License-Identifier: AGPL-3.0-or-later
//! Ranked attempts and the leaderboards (docs/leaderboards.md): one ranked
//! attempt per player per live daily, which only ends once; later attempts are
//! the player's own; the boards.

use super::*;
use rusqlite::{params, OptionalExtension};

/// c2-gate is a live daily dated 2026-11-27.
const DAILY: &str = "c2-gate";

async fn start(s: &Site, cookie: &str) -> String {
    let r = s
        .send(
            Request::post(format!("/api/p/{DAILY}/instance"))
                .header(header::COOKIE, cookie)
                .header(header::ORIGIN, ORIGIN)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let url = r
        .body
        .split("\"url\":\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    url.strip_prefix("https://")
        .unwrap()
        .strip_suffix(&format!(".{LIVE_DOMAIN}"))
        .unwrap()
        .to_owned()
}

async fn stop(s: &Site, cookie: &str) {
    let r = s
        .send(
            Request::post(format!("/api/p/{DAILY}/instance/stop"))
                .header(header::COOKIE, cookie)
                .header(header::ORIGIN, ORIGIN)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
}

/// Sends the flag; returns the answer's JSON.
async fn answer(s: &Site, cookie: &str, flag: &str) -> String {
    let r = s
        .post_json(
            &format!("/api/p/{DAILY}/answer"),
            &format!(r#"{{"answer":"{flag}"}}"#),
            Some(cookie),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    r.body
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

/// (status, seconds) of the player's ranked attempt, and how many personal records they have.
async fn attempts(s: &Site, name: &str) -> (Option<(String, Option<i64>)>, i64) {
    let id = uid(s, name).await;
    s.state.db.attempts_settle(None).await.unwrap();
    s.state
        .db
        .run(move |c| {
            let ranked = c
                .query_row(
                    "SELECT status, seconds FROM ranked_attempts WHERE user_id = ?1 AND pack_id = ?2",
                    params![id, DAILY],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let personal = c.query_row(
                "SELECT COUNT(*) FROM personal_records WHERE user_id = ?1 AND pack_id = ?2",
                params![id, DAILY],
                |r| r.get(0),
            )?;
            Ok((ranked, personal))
        })
        .await
        .unwrap()
}

/// Moves the player's ranked attempt's start back, so its time is not zero.
async fn started_ago(s: &Site, name: &str, secs: i64) {
    let id = uid(s, name).await;
    s.state
        .db
        .run(move |c| {
            c.execute_batch("DROP TRIGGER ranked_attempts_final")?;
            c.execute(
                "UPDATE ranked_attempts SET started_at = started_at - ?2 WHERE user_id = ?1",
                params![id, secs],
            )?;
            c.execute_batch(&format!(
                "CREATE TRIGGER ranked_attempts_final {}",
                RANKED_TRIGGER_BODY
            ))
        })
        .await
        .unwrap();
}

const RANKED_TRIGGER_BODY: &str = "
BEFORE UPDATE OF user_id, pack_id, started_at, ended_at, status, seconds, hints ON ranked_attempts
WHEN OLD.status != 'running'
  OR NEW.user_id != OLD.user_id OR NEW.pack_id != OLD.pack_id
  OR NEW.started_at != OLD.started_at
  OR NEW.status NOT IN ('solved', 'failed')
BEGIN
    SELECT RAISE(ABORT, 'a ranked attempt only ends once');
END;";

#[tokio::test]
async fn the_first_instance_on_its_day_is_the_one_ranked_attempt() {
    let s = live_site_on("2026-11-27").await;
    let (alice, label) = spin_up(&s, "alice", DAILY).await;
    assert_eq!(
        attempts(&s, "alice").await,
        (Some(("running".into(), None)), 0)
    );
    let page = s.get(&format!("/p/{DAILY}"), Some(&alice)).await.body;
    assert!(page.contains("Your ranked attempt is running."), "{page}");

    started_ago(&s, "alice", 125).await;
    let flag = pebble_flag(&s, &label).await;
    let r = answer(&s, &alice, &flag).await;
    assert!(
        r.contains(r#""correct":true"#) && r.contains(r#""attempt":"ranked""#),
        "{r}"
    );
    let (ranked, personal) = attempts(&s, "alice").await;
    let (status, secs) = ranked.unwrap();
    assert_eq!((status.as_str(), personal), ("solved", 0));
    assert!((125..=130).contains(&secs.unwrap()), "{secs:?}");

    // Another go: a personal record, solved, and the ranked attempt is unchanged.
    let label2 = start(&s, &alice).await;
    assert_ne!(label, label2);
    let flag2 = pebble_flag(&s, &label2).await;
    let r = answer(&s, &alice, &flag2).await;
    assert!(r.contains(r#""attempt":"personal""#), "{r}");
    let (ranked2, personal2) = attempts(&s, "alice").await;
    assert_eq!(ranked2.unwrap().1, secs, "the ranked time never changes");
    assert_eq!(personal2, 1);

    // The board has one entry for her, with the ranked time.
    let board = s.get("/leaderboard/ranked", None).await.body;
    assert_eq!(board.matches(">alice<").count(), 1, "{board}");
    assert!(board.contains("2:0"), "{board}");
    // Her account page lists both; nobody else's page or board shows the personal one.
    let account = s.get("/account", Some(&alice)).await.body;
    assert!(
        account.contains("Ranked") && account.contains("Personal"),
        "{account}"
    );
}

#[tokio::test]
async fn stopping_or_running_out_ends_the_ranked_attempt_as_failed() {
    let s = live_site_on("2026-11-27").await;
    let (bob, _) = spin_up(&s, "bob", DAILY).await;
    stop(&s, &bob).await;
    assert_eq!(attempts(&s, "bob").await.0, Some(("failed".into(), None)));
    // Starting again does not give a second ranked attempt.
    let label = start(&s, &bob).await;
    let flag = pebble_flag(&s, &label).await;
    assert!(answer(&s, &bob, &flag)
        .await
        .contains(r#""attempt":"personal""#));
    assert_eq!(attempts(&s, "bob").await.0.unwrap().0, "failed");
    assert!(!s
        .get("/leaderboard/ranked", None)
        .await
        .body
        .contains(">bob<"));

    // Running out: the instance ends, and its flag sent in later still solves
    // the puzzle but is not a ranked solve.
    let (carol, label) = spin_up(&s, "carol", DAILY).await;
    let flag = pebble_flag(&s, &label).await;
    s.state
        .db
        .live_set_expiry(label, crate::db::now() - 1)
        .await
        .unwrap();
    let r = answer(&s, &carol, &flag).await;
    assert!(r.contains(r#""correct":true"#), "{r}");
    assert!(!r.contains("attempt"), "{r}");
    assert_eq!(attempts(&s, "carol").await.0.unwrap().0, "failed");
}

#[tokio::test]
async fn an_attempt_off_its_day_is_personal() {
    // The day after: c2-gate is in the archive and playable, but not ranked.
    let s = live_site_on("2026-11-28").await;
    let (dave, label) = spin_up(&s, "dave", DAILY).await;
    assert_eq!(attempts(&s, "dave").await, (None, 1));
    let page = s.get(&format!("/p/{DAILY}"), Some(&dave)).await.body;
    assert!(page.contains("only made on the puzzle"), "{page}");
    let flag = pebble_flag(&s, &label).await;
    assert!(answer(&s, &dave, &flag)
        .await
        .contains(r#""attempt":"personal""#));
    assert!(!s
        .get("/leaderboard/ranked", None)
        .await
        .body
        .contains(">dave<"));
}

#[tokio::test]
async fn the_database_refuses_to_reopen_or_retime_a_finished_attempt() {
    let s = live_site_on("2026-11-27").await;
    let (erin, label) = spin_up(&s, "erin", DAILY).await;
    let flag = pebble_flag(&s, &label).await;
    answer(&s, &erin, &flag).await;
    let id = uid(&s, "erin").await;
    for sql in [
        "UPDATE ranked_attempts SET status = 'running' WHERE user_id = ?1",
        "UPDATE ranked_attempts SET seconds = 1 WHERE user_id = ?1",
        "UPDATE ranked_attempts SET status = 'failed' WHERE user_id = ?1",
        "UPDATE ranked_attempts SET started_at = 0 WHERE user_id = ?1",
    ] {
        let e = s
            .state
            .db
            .run(move |c| c.execute(sql, [id]))
            .await
            .unwrap_err();
        assert!(e.to_string().contains("only ends once"), "{sql}: {e}");
    }
    // Hiding is allowed, and leaves the attempt as it was.
    assert!(s
        .state
        .db
        .ranked_hide(id, DAILY.into(), id, Some("test".into()))
        .await
        .unwrap());
    assert_eq!(attempts(&s, "erin").await.0.unwrap().0, "solved");
}

#[tokio::test]
async fn a_start_that_failed_leaves_no_attempt() {
    let s = live_site_on("2026-11-27").await;
    s.register_verified("fay", "fay@example.org").await;
    let id = uid(&s, "fay").await;
    let t = crate::db::now();
    let inst = crate::db::Instance {
        label: "fail-start-one".into(),
        user_id: id,
        pack_id: DAILY.into(),
        seed: "s".into(),
        created_at: t,
        expires_at: t + 600,
        flags: Vec::new(),
        keyed: true,
    };
    let kind = s
        .state
        .db
        .live_create_attempt(inst.clone(), Some(true))
        .await
        .unwrap();
    assert_eq!(kind, Some(crate::db::Attempt::Ranked));
    s.state.db.live_create_failed(inst).await.unwrap();
    assert_eq!(attempts(&s, "fay").await, (None, 0));
    // So the next start is still the ranked one.
    let cookie = s.login_cookie("fay").await;
    start(&s, &cookie).await;
    assert_eq!(attempts(&s, "fay").await.0.unwrap().0, "running");
}

#[tokio::test]
async fn only_an_admin_hides_an_entry_and_it_keeps_its_time() {
    let s = live_site_on("2026-11-27").await;
    let (gus, label) = spin_up(&s, "gus", DAILY).await;
    let flag = pebble_flag(&s, &label).await;
    answer(&s, &gus, &flag).await;
    let id = uid(&s, "gus").await;
    let hide = format!("/leaderboard/ranked/{DAILY}/{id}/hide");

    // A player gets a 404, a visitor the login page; nothing changes.
    let (hal, _) = spin_up(&s, "hal", DAILY).await;
    assert_eq!(
        s.post(&hide, &[("reason", "x")], Some(&hal)).await.status,
        StatusCode::NOT_FOUND
    );
    let anon = s.post(&hide, &[("reason", "x")], None).await;
    assert!(
        anon.headers
            .get(header::LOCATION)
            .and_then(|l| l.to_str().ok())
            .is_some_and(|l| l.starts_with("/login")),
        "{:?}",
        anon.status
    );
    assert!(s
        .get("/leaderboard/ranked", None)
        .await
        .body
        .contains(">gus<"));

    let admin = s.admin_cookie().await;
    assert_eq!(
        s.post(&hide, &[("reason", " ")], Some(&admin)).await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        s.post(&hide, &[("reason", "shared flag")], Some(&admin))
            .await
            .status,
        StatusCode::SEE_OTHER
    );
    assert!(!s
        .get("/leaderboard/ranked", None)
        .await
        .body
        .contains(">gus<"));
    let seen = s.get("/leaderboard/ranked", Some(&admin)).await.body;
    assert!(
        seen.contains(">gus<") && seen.contains("shared flag"),
        "{seen}"
    );
    let account = s.get("/account", Some(&gus)).await.body;
    assert!(account.contains("hidden by an admin"), "{account}");
    assert_eq!(attempts(&s, "gus").await.0.unwrap().0, "solved");

    let show = format!("/leaderboard/ranked/{DAILY}/{id}/show");
    assert_eq!(
        s.post(&show, &[], Some(&admin)).await.status,
        StatusCode::SEE_OTHER
    );
    assert!(s
        .get("/leaderboard/ranked", None)
        .await
        .body
        .contains(">gus<"));
    // No such attempt.
    assert_eq!(
        s.post(
            &format!("/leaderboard/ranked/{DAILY}/99999/hide"),
            &[("reason", "x")],
            Some(&admin)
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn the_ranked_board_is_fastest_first_and_only_for_live_dailies() {
    let s = live_site_on("2026-11-27").await;
    for (name, ago) in [("ivy", 300), ("jon", 60)] {
        let (cookie, label) = spin_up(&s, name, DAILY).await;
        started_ago(&s, name, ago).await;
        let flag = pebble_flag(&s, &label).await;
        answer(&s, &cookie, &flag).await;
    }
    let board = s.get("/leaderboard/ranked", None).await.body;
    let (jon, ivy) = (board.find(">jon<").unwrap(), board.find(">ivy<").unwrap());
    assert!(jon < ivy, "the faster first");
    assert!(board.contains("2 solved of 2 ranked attempts"), "{board}");
    // A puzzle that is not a live daily has no board.
    assert_eq!(
        s.get("/leaderboard/ranked?pack=iloveyou-dropped", None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn the_casual_board_ranks_streaks_then_solves() {
    // 2026-11-26: the three ILOVEYOU dailies (24th to 26th) are out.
    let s = site_on("2026-11-26").await;
    s.register_verified("kim", "kim@example.org").await;
    s.register_verified("lee", "lee@example.org").await;
    s.register_verified("max", "max@example.org").await;
    let day = |d: i64| necropsy_pack::days_from_civil(2026, 11, d) * 86_400 + 12 * 3600;
    let solves: Vec<(&str, &str, i64)> = vec![
        // kim: each daily on its day, a 3-day streak.
        ("kim", "iloveyou-dropped", day(24)),
        ("kim", "iloveyou-attachment", day(25)),
        ("kim", "iloveyou-payload", day(26)),
        // lee: more solves, but only today's daily on its day.
        ("lee", "iloveyou-payload", day(26)),
        ("lee", "intro-log", day(26)),
        ("lee", "intro-javascript", day(26)),
        ("lee", "iloveyou-dropped", day(26)),
        // max: dailies solved late make no streak.
        ("max", "iloveyou-dropped", day(26)),
    ];
    for (name, pack, at) in solves {
        let id = uid(&s, name).await;
        let pack = pack.to_owned();
        s.state
            .db
            .run(move |c| {
                c.execute(
                    "INSERT INTO solves (user_id, pack_id, solved_at, seconds, hints) VALUES (?1, ?2, ?3, 60, 0)",
                    params![id, pack, at],
                )
            })
            .await
            .unwrap();
    }
    let rows = s.state.db.casual_rows().await.unwrap();
    let e = crate::leaderboard::casual_entries(&s.state, rows);
    let got: Vec<(&str, i64, i64)> = e
        .iter()
        .map(|e| (e.username.as_str(), e.streak, e.solved))
        .collect();
    assert_eq!(got, vec![("kim", 3, 3), ("lee", 1, 4), ("max", 0, 1)]);
    let page = s.get("/leaderboard", None).await.body;
    assert!(page.find(">kim<").unwrap() < page.find(">lee<").unwrap());
    // Nav link.
    assert!(page.contains(r#"href="/leaderboard""#));
}
