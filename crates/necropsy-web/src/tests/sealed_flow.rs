// SPDX-License-Identifier: AGPL-3.0-or-later
//! Sealed packs: opened with the master key, checked, served from their date;
//! a broken one replaced by a fallback; keys published after New York midnight.

use super::*;
use necropsy_pack::seal::{self, Manifest, MasterKeys};

fn master() -> MasterKeys {
    MasterKeys::parse(&format!("1:{}", "5a".repeat(32))).unwrap()
}

/// A small daily pack's files. `date: None` makes a fallback (no date).
fn daily(id: &str, number: u32, date: Option<&str>, answer: &str) -> Vec<(String, Vec<u8>)> {
    let date_line = date
        .map(|d| format!("date = \"{d}\"\n"))
        .unwrap_or_default();
    vec![
        (
            "pack.toml".into(),
            format!(
                "id = \"{id}\"\ntrack = \"daily\"\n{date_line}number = {number}\ntitle = \"Sealed {id}\"\ndifficulty = 2\nmodes = [\"web\"]\n"
            )
            .into_bytes(),
        ),
        ("brief.md".into(), b"What is the word?".to_vec()),
        ("explainer.md".into(), b"It was in the file.".to_vec()),
        ("files/note.txt".into(), format!("the word is {answer}\n").into_bytes()),
        ("answer".into(), format!("{}\n", necropsy_pack::hash_answer(id, answer)).into_bytes()),
    ]
}

struct Fixture {
    dir: std::path::PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A packs-sealed directory: `alpha` (2026-12-20), `broken` (2026-12-21, whose
/// file no longer matches its manifest) and the fallback `spare`.
fn fixture(name: &str) -> Fixture {
    let dir = std::env::temp_dir().join(format!("necropsy-sealed-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("wrap-keys")).unwrap();
    let wk = master().current_wrap_key();
    std::fs::write(dir.join("wrap-keys/v1.pub"), wk.to_file()).unwrap();
    let mut m = Manifest::default();
    for (id, number, date, fallback) in [
        ("alpha", 50, Some("2026-12-20"), false),
        ("broken", 51, Some("2026-12-21"), false),
        ("spare", 99, None, true),
    ] {
        let (bytes, _) =
            seal::seal(id, &daily(id, number, date, &format!("{id}word")), &wk).unwrap();
        std::fs::write(dir.join(format!("{id}.sealed")), &bytes).unwrap();
        m.upsert(seal::Entry {
            id: id.into(),
            number,
            difficulty: 2,
            date: date.map(str::to_owned),
            fallback,
            key_version: 1,
            sha256: seal::sha256_hex(&bytes),
        });
    }
    std::fs::write(dir.join("manifest.toml"), m.to_toml()).unwrap();
    Fixture { dir }
}

fn break_one(f: &Fixture) {
    let p = f.dir.join("broken.sealed");
    let mut b = std::fs::read(&p).unwrap();
    let n = b.len();
    b[n - 1] ^= 1;
    std::fs::write(&p, b).unwrap();
}

/// A site with the repository's packs and the fixture's sealed ones.
fn site_with_sealed(f: &Fixture, master: Option<&MasterKeys>, today: &str, now: i64) -> Site {
    let root = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
    let mut packs = necropsy_pack::load_all(&root.join("packs")).unwrap();
    let (sealed_packs, sealed_state) = crate::sealed::load(&f.dir, master, &packs, &mut |w, s| {
        necropsy_pack::gen::run(w, s)
    });
    packs.extend(sealed_packs);
    let mail = Arc::new(Mutex::new(Vec::new()));
    let mut state = AppState::new(
        packs,
        Config {
            base_url: ORIGIN.into(),
            client_ip_header: None,
            trusted_proxies: Vec::new(),
            live: None,
            captcha: crate::captcha::Captcha::Off,
            log_requests: false,
        },
        Db::open(":memory:").unwrap(),
        Mailer::Memory(mail.clone()),
    );
    state.today_override = Some(necropsy_pack::parse_date(today).unwrap());
    state.clock_override = Some(now);
    state.sealed = sealed_state;
    let state = Arc::new(state);
    Site {
        app: router(state.clone()),
        state,
        mail,
    }
}

fn at(date: &str, hour: i64) -> i64 {
    necropsy_pack::parse_date(date).unwrap() * 86_400 + hour * 3600
}

#[tokio::test]
async fn a_sealed_daily_is_served_from_its_date_and_a_fallback_replaces_a_broken_one() {
    let f = fixture("serve");
    break_one(&f);
    let m = master();
    let before = site_with_sealed(&f, Some(&m), "2026-12-19", at("2026-12-19", 12));
    assert_eq!(
        before.get("/p/alpha", None).await.status,
        StatusCode::NOT_FOUND
    );
    let st = &before.state.sealed.status;
    let outcome = |id: &str| {
        st.iter()
            .find(|s| s.id == id)
            .map(|s| s.outcome.clone())
            .unwrap_or_default()
    };
    assert_eq!(outcome("alpha"), "ok");
    assert!(
        outcome("broken").contains("does not match its manifest sha256"),
        "{st:?}"
    );
    assert_eq!(outcome("spare"), "fallback for broken");

    let on = site_with_sealed(&f, Some(&m), "2026-12-20", at("2026-12-20", 12));
    let page = on.get("/p/alpha", None).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("Sealed alpha"));
    assert!(on
        .post_json("/api/p/alpha/answer", r#"{"answer":"alphaword"}"#, None)
        .await
        .body
        .contains("\"correct\":true"));
    // The fallback is the next day's daily, with the broken one's number.
    let next = site_with_sealed(&f, Some(&m), "2026-12-21", at("2026-12-21", 12));
    let page = next.get("/p/spare", None).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(
        page.body.contains("Daily #51"),
        "{}",
        &page.body[..400.min(page.body.len())]
    );
    assert_eq!(
        next.get("/p/broken", None).await.status,
        StatusCode::NOT_FOUND
    );
    // The admin sees what happened.
    let admin = next.admin_cookie().await;
    let page = next.get("/admin", Some(&admin)).await.body;
    assert!(page.contains("Sealed packs") && page.contains("fallback for broken"));
}

#[tokio::test]
async fn keys_are_published_after_new_york_midnight_and_open_only_their_pack() {
    let f = fixture("keys");
    let m = master();
    // 2026-12-20 is EST: its key is due at 2026-12-21 05:00 UTC.
    let early = site_with_sealed(&f, Some(&m), "2026-12-21", at("2026-12-21", 5) - 1);
    let r = early.get("/api/packs/alpha/key", None).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert!(
        r.body.contains("not released yet") && r.body.contains("2026-12-21 05:00 UTC"),
        "{}",
        r.body
    );
    let list = early.get("/api/packs/keys", None).await.body;
    assert!(list.contains("\"released\":[]"), "{list}");
    assert!(
        list.contains("\"upcoming\":[{\"date\":\"2026-12-20\""),
        "{list}"
    );

    let late = site_with_sealed(&f, Some(&m), "2026-12-21", at("2026-12-21", 5));
    let r = late.get("/api/packs/alpha/key", None).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
    let k = seal::key_from_hex(v["key"].as_str().unwrap()).unwrap();
    // It opens alpha from the repository's file, and nothing else.
    let alpha = seal::parse(&std::fs::read(f.dir.join("alpha.sealed")).unwrap()).unwrap();
    assert!(seal::open(&alpha, &k).is_ok());
    let spare = seal::parse(&std::fs::read(f.dir.join("spare.sealed")).unwrap()).unwrap();
    assert!(seal::open(&spare, &k).is_err());
    assert!(late
        .get("/api/packs/keys", None)
        .await
        .body
        .contains("\"id\":\"alpha\""));
    // Unknown, and the fallback nobody needed: no key.
    assert_eq!(
        late.get("/api/packs/nope/key", None).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        late.get("/api/packs/spare/key", None).await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn the_wrap_key_endpoint_and_a_site_without_a_master_key() {
    let f = fixture("wrap");
    let m = master();
    let s = site_with_sealed(&f, Some(&m), "2026-12-19", at("2026-12-19", 0));
    let r = s.get("/api/packs/wrap-key", None).await;
    assert_eq!(r.status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(seal::WrapKey::from_json(&v).unwrap(), m.current_wrap_key());
    assert!(!r.body.contains(&"5a".repeat(32)), "never the master key");

    let none = site_with_sealed(&f, None, "2026-12-20", at("2026-12-20", 12));
    assert_eq!(
        none.get("/api/packs/wrap-key", None).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        none.get("/p/alpha", None).await.status,
        StatusCode::NOT_FOUND
    );
    assert!(none
        .state
        .sealed
        .status
        .iter()
        .all(|s| s.outcome.contains("no master key")));
}

#[tokio::test]
async fn another_sites_master_key_opens_nothing() {
    let f = fixture("other");
    let other = MasterKeys::parse(&"77".repeat(32)).unwrap();
    let s = site_with_sealed(&f, Some(&other), "2026-12-20", at("2026-12-20", 12));
    assert_eq!(s.get("/p/alpha", None).await.status, StatusCode::NOT_FOUND);
    assert!(s
        .state
        .sealed
        .status
        .iter()
        .any(|x| x.outcome.contains("sealed to another v1 key")));
}

/// pack-tool's client reads what the site serves.
#[tokio::test]
async fn pack_tool_fetches_the_wrap_key_and_released_keys_from_a_site() {
    let f = fixture("client");
    let m = master();
    let s = site_with_sealed(&f, Some(&m), "2026-12-22", at("2026-12-22", 12));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = s.app.clone();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap()
    });
    let site = format!("http://{addr}");
    let (wk, k) = tokio::task::spawn_blocking(move || {
        (
            necropsy_pack::site::fetch_wrap_key(&site),
            necropsy_pack::site::fetch_released_key(&site, "alpha"),
        )
    })
    .await
    .unwrap();
    assert_eq!(wk.unwrap(), m.current_wrap_key());
    let alpha = seal::parse(&std::fs::read(f.dir.join("alpha.sealed")).unwrap()).unwrap();
    assert!(seal::open(&alpha, &k.unwrap()).is_ok());
}

#[test]
fn ci_checks_the_sealed_directory_without_secrets() {
    let root = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
    let plain = necropsy_pack::load_all(&root.join("packs")).unwrap();
    let f = fixture("ci");
    let report = seal::check_dir(&f.dir, &plain).unwrap();
    assert_eq!(report.len(), 3);
    // A tampered file.
    break_one(&f);
    assert!(seal::check_dir(&f.dir, &plain)
        .unwrap_err()
        .contains("sha256 does not match"));
    let f = fixture("ci2");
    // A plaintext pack committed where sealed ones go.
    std::fs::write(f.dir.join("pack.toml"), "id = \"oops\"").unwrap();
    assert!(seal::check_dir(&f.dir, &plain)
        .unwrap_err()
        .contains("does not belong here"));
    let f = fixture("ci3");
    // A date a plaintext daily already has.
    let mut m = Manifest::load(&f.dir).unwrap();
    m.pack.iter_mut().find(|e| e.id == "alpha").unwrap().date = Some("2026-11-27".into());
    std::fs::write(f.dir.join("manifest.toml"), m.to_toml()).unwrap();
    assert!(seal::check_dir(&f.dir, &plain)
        .unwrap_err()
        .contains("already c2-gate's date"));
    let f = fixture("ci4");
    // Sealed to a key that is not the committed one.
    let stranger = MasterKeys::parse(&"66".repeat(32))
        .unwrap()
        .current_wrap_key();
    std::fs::write(f.dir.join("wrap-keys/v1.pub"), stranger.to_file()).unwrap();
    assert!(seal::check_dir(&f.dir, &plain)
        .unwrap_err()
        .contains("not the committed one"));
    let f = fixture("ci5");
    // A newer committed key: everything must be resealed to it.
    let v2 = MasterKeys::parse(&format!("2:{}", "44".repeat(32)))
        .unwrap()
        .current_wrap_key();
    std::fs::write(f.dir.join("wrap-keys/v2.pub"), v2.to_file()).unwrap();
    assert!(seal::check_dir(&f.dir, &plain)
        .unwrap_err()
        .contains("current wrap key is v2"));
}

/// The repository's own packs-sealed/ passes the same checks.
#[test]
fn the_repository_sealed_directory_is_valid() {
    let root = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
    let plain = necropsy_pack::load_all(&root.join("packs")).unwrap();
    seal::check_dir(&root.join("packs-sealed"), &plain).unwrap();
}

/// Pull requests run with no secrets: no workflow uses `pull_request_target`
/// (which runs a PR's code with the base repository's secrets), and a workflow
/// that pull requests trigger never reads one. See docs/ci-cd.md.
#[test]
fn pull_request_workflows_never_see_a_secret() {
    let dir = std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../.github/workflows"
    ));
    let mut seen = 0;
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(
            !text.contains("pull_request_target"),
            "{} uses pull_request_target",
            p.display()
        );
        let on_pr = text
            .lines()
            .any(|l| l.trim_start().starts_with("pull_request"));
        if on_pr {
            seen += 1;
            assert!(
                !text.contains("secrets."),
                "{} runs on pull requests and reads a secret",
                p.display()
            );
        }
    }
    assert!(seen >= 1, "ci.yml runs on pull requests");
}

#[test]
fn the_master_key_is_made_when_there_is_none_and_kept() {
    let dir = std::env::temp_dir().join(format!("necropsy-master-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let _f = Fixture { dir: dir.clone() };
    let default = dir.join("master-key");

    // Nothing set: made at the default path, 0600, and the same key next time.
    let (a, made) = crate::sealed::load_master(None, None, &default).unwrap();
    assert!(made);
    let text = std::fs::read_to_string(&default).unwrap();
    assert!(
        text.starts_with("1:") && text.trim().len() == 66,
        "{text:?}"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&default).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let (b, made) = crate::sealed::load_master(Some(" ".into()), None, &default).unwrap();
    assert!(!made, "an existing key is kept");
    assert_eq!(a.current_wrap_key(), b.current_wrap_key());

    // The variable is used, and nothing is made, when no file is named.
    let given = format!("1:{}", "5a".repeat(32));
    let other = dir.join("unused");
    let (c, made) = crate::sealed::load_master(None, Some(given.clone()), &other).unwrap();
    assert!(!made && !other.exists());
    assert_eq!(c.current_wrap_key(), master().current_wrap_key());

    // A named file wins over the variable, and is made if missing.
    let named = dir.join("named.key");
    let (d, made) =
        crate::sealed::load_master(Some(named.display().to_string()), Some(given), &other).unwrap();
    assert!(made && named.exists());
    assert_ne!(d.current_wrap_key(), master().current_wrap_key());

    // A malformed key is refused, never replaced.
    std::fs::write(&default, "1:abc\n").unwrap();
    let e = crate::sealed::load_master(None, None, &default).unwrap_err();
    assert!(e.contains("not 64 hex digits"), "{e}");
    assert_eq!(std::fs::read_to_string(&default).unwrap(), "1:abc\n");
}
