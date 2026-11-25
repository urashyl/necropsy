// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tests against a fake `docker`: a script that logs its arguments, opens the
//! container's socket (a file, here) when asked to run one, and answers `ps`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use super::*;

static N: AtomicU32 = AtomicU32::new(0);

struct Rig {
    app: Arc<App>,
    dir: PathBuf,
}

const FAKE_DOCKER: &str = r#"#!/bin/sh
here="$(dirname "$0")"
echo "$@" >> "$here/log"
case "$1" in
  run)
    for a in "$@"; do
      case "$a" in --volume=*) v="${a#--volume=}"; touch "${v%%:*}/app.sock";; esac
      case "$a" in --name=nr-failme*) echo "boom" >&2; exit 1;; esac
    done
    echo abc123;;
  ps) cat "$here/ps" 2>/dev/null || true;;
esac
exit 0
"#;

fn rig(max: usize) -> Rig {
    let dir = std::env::temp_dir().join(format!(
        "necropsy-spawner-test-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("range")).unwrap();
    let docker = dir.join("docker");
    std::fs::write(&docker, FAKE_DOCKER).unwrap();
    std::fs::set_permissions(&docker, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (_, images) = config::parse_images(&format!(
        r#"
        max_instances = 5
        [images.lab]
        image = "necropsy-range/lab:1"
        uid = {}
        "#,
        test_uid()
    ))
    .unwrap();
    let cfg = Config {
        socket: dir.join("s.sock"),
        socket_gid: 0,
        range_dir: dir.join("range"),
        docker: docker.display().to_string(),
        max_instances: max,
        images,
    };
    Rig {
        app: App::new(cfg.clone(), Docker::new(cfg.docker)),
        dir,
    }
}

impl Rig {
    async fn call(&self, method: &str, uri: &str, body: &str) -> (StatusCode, String) {
        let res = router(self.app.clone())
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_owned()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.dir.join("log")).unwrap_or_default()
    }

    async fn make(&self, id: &str) -> (StatusCode, String) {
        self.call(
            "POST",
            "/v1/instances",
            &format!(
                r#"{{"id":"{id}","image":"lab","ttl_secs":600,"env":{{"NECROPSY_FLAG":"kit{{abc}}"}}}}"#
            ),
        )
        .await
    }
}

fn sock(r: &Rig, id: &str) -> PathBuf {
    r.dir.join("range").join(id).join("app.sock")
}

/// The uid the test images run as: root can chown to anything, anyone else
/// only to themselves.
fn test_uid() -> u32 {
    use std::os::unix::fs::MetadataExt;
    match std::fs::metadata(".").unwrap().uid() {
        0 => 4242,
        own => own,
    }
}

#[tokio::test]
async fn an_instance_is_started_locked_down_and_stopped() {
    let r = rig(5);
    let (s, b) = r.make("abcde12345").await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    assert!(sock(&r, "abcde12345").exists());

    let log = r.log();
    for must in [
        "--network=none",
        "--log-driver=none",
        "--read-only",
        "--cap-drop=ALL",
        "--security-opt=no-new-privileges:true",
        "--memory=128m",
        "--memory-swap=128m",
        "--pids-limit=64",
        "--name=nr-abcde12345",
        "--env=NECROPSY_FLAG=kit{abc}",
        "noexec",
    ] {
        assert!(log.contains(must), "missing {must}: {log}");
    }
    assert!(
        log.trim_end().ends_with("necropsy-range/lab:1"),
        "the image comes last: {log}"
    );
    assert!(!log.contains("--privileged") && !log.contains("docker.sock"));

    // The same id twice is refused; listing shows it; touch extends it.
    assert_eq!(r.make("abcde12345").await.0, StatusCode::CONFLICT);
    assert!(r
        .call("GET", "/v1/instances", "")
        .await
        .1
        .contains("abcde12345"));
    let (s, _) = r
        .call(
            "POST",
            "/v1/instances/abcde12345/touch",
            r#"{"ttl_secs":3000}"#,
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert!(r.app.lock()["abcde12345"].expires > now() + 2900);
    assert_eq!(
        r.call("POST", "/v1/instances/nope/touch", r#"{"ttl_secs":5}"#)
            .await
            .0,
        StatusCode::NOT_FOUND
    );

    assert_eq!(
        r.call("DELETE", "/v1/instances/abcde12345", "").await.0,
        StatusCode::OK
    );
    assert!(!sock(&r, "abcde12345").exists());
    assert!(r.log().contains("rm -f nr-abcde12345"));
    assert!(r.app.lock().is_empty());
}

#[tokio::test]
async fn bad_requests_are_refused_before_docker_is_run() {
    let r = rig(5);
    for (why, body) in [
        (
            "unknown image",
            r#"{"id":"abc","image":"evil","ttl_secs":60}"#,
        ),
        ("bad id", r#"{"id":"../x","image":"lab","ttl_secs":60}"#),
        ("empty id", r#"{"id":"","image":"lab","ttl_secs":60}"#),
        ("zero ttl", r#"{"id":"abc","image":"lab","ttl_secs":0}"#),
        (
            "huge ttl",
            r#"{"id":"abc","image":"lab","ttl_secs":999999}"#,
        ),
        (
            "other env",
            r#"{"id":"abc","image":"lab","ttl_secs":60,"env":{"LD_PRELOAD":"x"}}"#,
        ),
        (
            "env value",
            r#"{"id":"abc","image":"lab","ttl_secs":60,"env":{"NECROPSY_FLAG":"a b;c"}}"#,
        ),
    ] {
        let (s, _) = r.call("POST", "/v1/instances", body).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{why}");
    }
    assert_eq!(r.log(), "", "docker was never run");
    assert_eq!(
        r.call("DELETE", "/v1/instances/Bad..id", "").await.0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn the_range_has_a_size_and_failures_clean_up() {
    let r = rig(2);
    assert_eq!(r.make("aaaaaa").await.0, StatusCode::CREATED);
    assert_eq!(r.make("bbbbbb").await.0, StatusCode::CREATED);
    assert_eq!(r.make("cccccc").await.0, StatusCode::TOO_MANY_REQUESTS);
    // A container that fails to start leaves nothing behind and frees its slot.
    r.call("DELETE", "/v1/instances/bbbbbb", "").await;
    let (s, _) = r.make("failme1").await;
    assert_eq!(s, StatusCode::BAD_GATEWAY);
    assert!(!r.dir.join("range/failme1").exists());
    assert_eq!(r.app.lock().len(), 1);
}

#[tokio::test]
async fn expired_instances_are_reaped_and_survivors_recovered() {
    let r = rig(5);
    let t = now();
    std::fs::write(
        r.dir.join("ps"),
        format!(
            "nr-old1 lab {}\nnr-live1 lab {}\nstranger x y\n",
            t - 5,
            t + 500
        ),
    )
    .unwrap();
    std::fs::create_dir_all(r.dir.join("range/old1")).unwrap();
    std::fs::create_dir_all(r.dir.join("range/live1")).unwrap();
    std::fs::create_dir_all(r.dir.join("range/orphan")).unwrap();
    r.app.recover().await;
    assert!(r.log().contains("rm -f nr-old1"));
    assert!(r.app.lock().contains_key("live1") && !r.app.lock().contains_key("old1"));
    assert!(r.dir.join("range/live1").exists());
    assert!(!r.dir.join("range/old1").exists());
    assert!(
        !r.dir.join("range/orphan").exists(),
        "an unowned directory is removed"
    );

    r.app.lock().get_mut("live1").unwrap().expires = t - 1;
    r.app.reap().await;
    assert!(r.app.lock().is_empty());
    assert!(!Path::new(&r.dir.join("range/live1")).exists());
}

#[test]
fn image_limits_are_checked() {
    for bad in [
        "[images.a]\nimage = \"x\"\nuid = 0",
        "[images.a]\nimage = \"-v\"",
        "[images.A]\nimage = \"x\"",
        "[images.a]\nimage = \"x\"\nmemory_mb = 8",
        "[images.a]\nimage = \"x\"\nmemory_mb = 64\ntmpfs_mb = 64",
        "[images.a]\nimage = \"x\"\nsurprise = 1",
    ] {
        assert!(config::parse_images(bad).is_err(), "{bad}");
    }
}

/// Every container-backed pack names an image the allowlist file has.
#[test]
fn packs_only_name_images_on_the_allowlist() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let text = std::fs::read_to_string(root.join("range/images.toml")).unwrap();
    let (_, images) = config::parse_images(&text).unwrap();
    for p in necropsy_pack::load_all(&root.join("packs")).unwrap() {
        if let Some(c) = p.live.as_ref().and_then(|l| l.container.as_ref()) {
            assert!(images.contains_key(&c.image), "{}: {}", p.meta.id, c.image);
        }
    }
}
