// SPDX-License-Identifier: AGPL-3.0-or-later
//! Share cards with streaks and hint squares, and Discord discussion of closed dailies.

use super::*;

/// Signs up `name` and records timed solves of `packs`, each made at noon UTC on
/// the day given (YYYY-MM-DD). Returns the session cookie and the user id.
async fn with_solves(s: &Site, name: &str, packs: &[(&str, &str, i64)]) -> (String, i64) {
    s.register_verified(name, &format!("{name}@example.org"))
        .await;
    let cookie = s.login_cookie(name).await;
    let uid = s
        .state
        .db
        .user_for_login(name.into())
        .await
        .unwrap()
        .unwrap()
        .0
        .id;
    for (pack, day, hints) in packs {
        s.state
            .db
            .record_solve(uid, (*pack).into(), 200, *hints, 0, true)
            .await
            .unwrap();
        let at = necropsy_pack::parse_date(day).unwrap() * 86_400 + 43_200;
        let pack = (*pack).to_owned();
        s.state
            .db
            .run(move |c| {
                c.execute(
                    "UPDATE solves SET solved_at = ?1 WHERE user_id = ?2 AND pack_id = ?3",
                    rusqlite::params![at, uid, pack],
                )
            })
            .await
            .unwrap();
    }
    (cookie, uid)
}

async fn share_of(s: &Site, cookie: &str, pack: &str) -> serde_json::Value {
    let r = s
        .post_json(&format!("/api/p/{pack}/share"), "{}", Some(cookie))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    serde_json::from_str(&r.body).unwrap()
}

#[tokio::test]
async fn a_daily_solved_on_its_day_shares_its_streak_and_squares() {
    let s = site().await;
    // Three dailies in a row, each on its own day.
    let (cookie, _) = with_solves(
        &s,
        "streaker",
        &[
            ("iloveyou-dropped", "2026-11-24", 0),
            ("iloveyou-attachment", "2026-11-25", 0),
            ("iloveyou-payload", "2026-11-26", 1),
        ],
    )
    .await;
    let out = share_of(&s, &cookie, "iloveyou-payload").await;
    let text = out["text"].as_str().unwrap();
    let url = out["url"].as_str().unwrap();
    let total = s
        .state
        .get("iloveyou-payload")
        .unwrap()
        .pack
        .meta
        .hints
        .len();
    let grid = format!("🟨{}", "🟩".repeat(total - 1));
    assert_eq!(
        text,
        format!(
            "necropsy Daily #3 · ILOVEYOU: the password stealer\n{grid} 3:20 · 1 of {total} hints · 3-day streak\n{url}"
        )
    );
    let token = url.rsplit('/').next().unwrap();
    let page = s.get(&format!("/card/{token}"), None).await.body;
    assert!(page.contains("3-day streak"), "{page}");
    assert!(
        page.contains("href=\"/learn\""),
        "new players are pointed at Learn"
    );
    let svg = s.get(&format!("/card/{token}.svg"), None).await.body;
    assert!(svg.contains(&format!(">1/{total}<")) && svg.contains("3-day streak"));
    // Never the answer or the player.
    assert!(!text.contains("streaker") && !page.contains("streaker"));
}

#[tokio::test]
async fn a_late_solve_has_no_streak() {
    let s = site().await;
    // Solved the day after its release: no streak line at all.
    let (cookie, _) = with_solves(
        &s,
        "latecomer",
        &[
            ("iloveyou-dropped", "2026-11-24", 0),
            ("iloveyou-attachment", "2026-11-26", 0),
        ],
    )
    .await;
    let text = share_of(&s, &cookie, "iloveyou-attachment").await["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(!text.contains("streak"), "{text}");
    // A single day on time is not called a streak either.
    let text = share_of(&s, &cookie, "iloveyou-dropped").await["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(!text.contains("streak"), "{text}");
}

#[tokio::test]
async fn the_puzzle_page_has_one_share_button_and_a_preview() {
    let page = site().await.get("/p/intro-log", None).await.body;
    assert!(page.contains("id=\"share-btn\"") && page.contains(">Share<"));
    assert!(page.contains("id=\"share-preview\""));
    assert!(!page.contains("Copy result") && !page.contains("share-card"));
    assert!(page.contains("data-label=\"Intro #2\""));
}

// ---- Discord ---------------------------------------------------------------------

fn with_community(s: Site, c: crate::community::Community) -> Site {
    let Site { app, state, mail } = s;
    drop(app);
    let mut state = Arc::try_unwrap(state).ok().expect("state is shared");
    state.community = c;
    let state = Arc::new(state);
    Site {
        app: router(state.clone()),
        state,
        mail,
    }
}

fn invite_only() -> crate::community::Community {
    crate::community::from_vars(|k| {
        (k == "NECROPSY_DISCORD_URL").then(|| "https://discord.gg/abc".into())
    })
    .unwrap()
}

#[tokio::test]
async fn settings_are_checked() {
    use crate::community::from_vars;
    let none = from_vars(|_| None).unwrap();
    assert!(!none.on());
    for (vars, want) in [
        (
            vec![("NECROPSY_DISCORD_URL", "http://discord.gg/x")],
            "https://",
        ),
        (
            vec![(
                "NECROPSY_DISCORD_WEBHOOK",
                "https://discord.com/api/webhooks/1/x",
            )],
            "both",
        ),
        (
            vec![
                (
                    "NECROPSY_DISCORD_WEBHOOK",
                    "https://discord.com/api/webhooks/1/x",
                ),
                ("NECROPSY_DISCORD_GUILD_ID", "abc"),
            ],
            "numeric",
        ),
        (
            vec![
                ("NECROPSY_DISCORD_WEBHOOK", "ftp://x"),
                ("NECROPSY_DISCORD_GUILD_ID", "1"),
            ],
            "webhook's URL",
        ),
    ] {
        let e = from_vars(|k| {
            vars.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        })
        .err()
        .unwrap();
        assert!(e.contains(want), "{e}");
    }
}

#[tokio::test]
async fn the_footer_links_discord_only_when_set() {
    assert!(!site()
        .await
        .get("/learn", None)
        .await
        .body
        .contains("discord.gg"));
    let s = with_community(site().await, invite_only());
    assert!(s
        .get("/learn", None)
        .await
        .body
        .contains("href=\"https://discord.gg/abc\""));
}

#[tokio::test]
async fn an_open_daily_has_no_discussion_link() {
    // iloveyou-payload is dated 2026-11-26: open on its day, closed the day after.
    let open = with_community(site_on("2026-11-26").await, invite_only());
    let page = open.get("/p/iloveyou-payload", None).await.body;
    assert!(!page.contains("Discuss"), "{page}");
    // A solver is told when it opens; app.js shows it after solving.
    assert!(page.contains("id=\"discuss-later\""));
    let closed = with_community(site_on("2026-11-27").await, invite_only());
    let page = closed.get("/p/iloveyou-payload", None).await.body;
    assert!(page.contains("Discuss closed dailies on Discord"), "{page}");
    // Not a daily: never.
    assert!(!closed
        .get("/p/intro-log", None)
        .await
        .body
        .contains("Discuss"));
    // Without Discord: nothing.
    let plain = site_on("2026-11-27").await;
    let page = plain.get("/p/iloveyou-payload", None).await.body;
    assert!(!page.contains("Discuss") && !page.contains("discuss-later"));
}

#[tokio::test]
async fn a_daily_closes_when_its_date_is_over_everywhere() {
    let Site { app, state, .. } = site().await;
    drop(app);
    let mut st = Arc::try_unwrap(state).ok().expect("state is shared");
    st.today_override = None;
    let day = necropsy_pack::parse_date("2026-11-26").unwrap();
    let id = "iloveyou-payload";
    // 2026-11-27 11:59:59 UTC is still the 26th in UTC-12.
    st.clock_override = Some((day + 1) * 86_400 + 12 * 3600 - 1);
    assert!(!st.is_closed(st.get_any(id).unwrap()));
    st.clock_override = Some((day + 1) * 86_400 + 12 * 3600);
    assert!(st.is_closed(st.get_any(id).unwrap()));
}

/// A stand-in for Discord's webhook: records each post and answers with a thread id.
async fn fake_discord(fail: bool) -> (String, Arc<Mutex<Vec<(String, serde_json::Value)>>>) {
    use axum::routing::post;
    let seen: Arc<Mutex<Vec<(String, serde_json::Value)>>> = Arc::default();
    let rec = seen.clone();
    let app = Router::new().route(
        "/api/webhooks/1/secret",
        post(
            move |q: axum::extract::RawQuery, axum::Json(v): axum::Json<serde_json::Value>| {
                let rec = rec.clone();
                async move {
                    let n = {
                        let mut r = rec.lock().unwrap();
                        r.push((q.0.unwrap_or_default(), v));
                        r.len()
                    };
                    if fail {
                        return (StatusCode::BAD_REQUEST, "nope".to_owned());
                    }
                    (
                        StatusCode::OK,
                        format!(r#"{{"id":"9","channel_id":"55500{n}"}}"#),
                    )
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}/api/webhooks/1/secret"), seen)
}

fn forum(url: &str) -> crate::community::Community {
    let url = url.to_owned();
    crate::community::from_vars(move |k| match k {
        "NECROPSY_DISCORD_WEBHOOK" => Some(url.clone()),
        "NECROPSY_DISCORD_GUILD_ID" => Some("4242".into()),
        _ => None,
    })
    .unwrap()
}

#[tokio::test]
async fn closed_dailies_get_a_thread_once_and_the_page_links_it() {
    let (url, seen) = fake_discord(false).await;
    // On 2026-11-28 the dailies of 11-24 to 11-27 have closed. c2-gate (11-27) is live
    // and hidden here, so three.
    let s = with_community(site_on("2026-11-28").await, forum(&url));
    let made = crate::community::post_due_threads(&s.state).await;
    assert_eq!(made, 3);
    {
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 3);
        for (q, body) in seen.iter() {
            assert_eq!(q, "wait=true");
            assert!(body["thread_name"].as_str().unwrap().starts_with("Daily #"));
            assert!(body["content"]
                .as_str()
                .unwrap()
                .contains("https://example.test/p/"));
            assert_eq!(body["allowed_mentions"]["parse"], serde_json::json!([]));
        }
        // The open daily (11-28, peanuts) is not among them.
        assert!(!seen
            .iter()
            .any(|(_, b)| b["content"].as_str().unwrap().contains("/p/peanuts")));
    }
    // Once is enough: a second run posts nothing, and the threads survive a restart.
    assert_eq!(crate::community::post_due_threads(&s.state).await, 0);
    assert_eq!(s.state.db.discussions().await.unwrap().len(), 3);
    let page = s.get("/p/iloveyou-dropped", None).await.body;
    assert!(
        page.contains("href=\"https://discord.com/channels/4242/55500"),
        "{page}"
    );
    assert!(page.contains("Discuss it on Discord"));
    assert!(!s.get("/p/peanuts", None).await.body.contains("Discuss it"));
}

#[tokio::test]
async fn old_dailies_are_not_backfilled_and_failures_retry() {
    let (url, seen) = fake_discord(true).await;
    // Much later: every daily closed more than 14 days ago, so nothing is posted.
    let s = with_community(site_on("2027-02-01").await, forum(&url));
    assert_eq!(crate::community::post_due_threads(&s.state).await, 0);
    assert!(seen.lock().unwrap().is_empty());
    // Discord refusing: nothing is recorded, and the next run tries again.
    let s = with_community(site_on("2026-11-28").await, forum(&url));
    assert_eq!(crate::community::post_due_threads(&s.state).await, 0);
    assert_eq!(seen.lock().unwrap().len(), 1, "stops at the first failure");
    assert!(s.state.db.discussions().await.unwrap().is_empty());
    assert_eq!(crate::community::post_due_threads(&s.state).await, 0);
    assert_eq!(seen.lock().unwrap().len(), 2);
}
