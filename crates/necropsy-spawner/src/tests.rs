// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tests against a fake `docker`: a script that logs its arguments, opens a lab's
//! socket (a file, here) when asked to run one, and answers the other commands
//! from files next to it.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tokio::net::UnixListener;
use tower::ServiceExt;

use super::*;

static N: AtomicU32 = AtomicU32::new(0);

const PROD_TOKEN: &str = "prod-token-0123456789abcdef";
const DEV_TOKEN: &str = "dev-token-0123456789abcdefgh";

const FAKE_DOCKER: &str = r#"#!/bin/sh
here="$(dirname "$0")"
echo "$@" >> "$here/log"
case "$1" in
  run)
    for a in "$@"; do
      case "$a" in --volume=*) v="${a#--volume=}"; touch "${v%%:*}/app.sock";; esac
      case "$a" in --name=nr-*-failme*) echo "boom" >&2; exit 1;; esac
      case "$a" in --name=*) echo "${a#--name=}" >> "$here/containers";; esac
    done
    echo abc123;;
  rm)
    # rm -f NAME: forget it, as docker would.
    grep -vx "$3" "$here/containers" > "$here/containers.new" 2>/dev/null; mv -f "$here/containers.new" "$here/containers" 2>/dev/null;;
  ps) cat "$here/ps" 2>/dev/null || true;;
  load) [ ! -e "$here/load_fail" ]; exit $?;;
  image)
    case "$2" in
      inspect) [ -e "$here/img_$(echo "$3" | tr '/:' '__')" ]; exit $?;;
      ls) cat "$here/imgls" 2>/dev/null || true;;
    esac;;
  inspect) grep -qx "$2" "$here/containers" 2>/dev/null || exit 1; cat "$here/inspect.json";;
  exec)
    shift; shift
    case "$*" in
      "cat /proc/net/dev") cat "$here/netdev";;
      "sh -c id -u") echo 10001;;
      "sh -c touch /selftest-write 2>/dev/null") exit 1;;
    esac;;
esac
exit 0
"#;

const GOOD_INSPECT: &str = r#"[{"HostConfig":{"NetworkMode":"none","ReadonlyRootfs":true,"Privileged":false,
 "CapDrop":["ALL"],"SecurityOpt":["no-new-privileges:true"],"Memory":134217728,"MemorySwap":134217728,
 "NanoCpus":500000000,"PidsLimit":64},"Config":{"User":"10001:10001"},"Mounts":[{"Source":"/run/x"}]}]"#;

const GOOD_NETDEV: &str = "Inter-|   Receive\n face |bytes\n    lo: 0 0\n";

struct Rig {
    app: Arc<App>,
    dir: PathBuf,
}

/// The uid the test labs run as: root can chown to anything, anyone else only
/// to themselves.
fn test_uid() -> u32 {
    use std::os::unix::fs::MetadataExt;
    match std::fs::metadata(".").unwrap().uid() {
        0 => 4242,
        own => own,
    }
}

fn token_hash(t: &str) -> String {
    hex(&Sha256::digest(t.as_bytes()))
}

fn rig() -> Rig {
    let dir = std::env::temp_dir().join(format!(
        "necropsy-spawner-test-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    for d in ["range", "state"] {
        std::fs::create_dir_all(dir.join(d)).unwrap();
    }
    let docker = dir.join("docker");
    std::fs::write(&docker, FAKE_DOCKER).unwrap();
    std::fs::set_permissions(&docker, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(dir.join("inspect.json"), GOOD_INSPECT).unwrap();
    std::fs::write(dir.join("netdev"), GOOD_NETDEV).unwrap();
    let tenants = config::parse_tenants(&format!(
        r#"
        [tenants.prod]
        token_sha256 = "{}"
        max_instances = 3
        memory_budget_mb = 300
        [tenants.dev]
        token_sha256 = "{}"
        max_instances = 2
        memory_budget_mb = 200
        "#,
        token_hash(PROD_TOKEN),
        token_hash(DEV_TOKEN)
    ))
    .unwrap();
    let cfg = Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        state_dir: dir.join("state"),
        range_dir: dir.join("range"),
        docker: docker.display().to_string(),
        tenants,
    };
    Rig {
        app: App::new(cfg.clone(), Docker::new(cfg.docker)),
        dir,
    }
}

impl Rig {
    async fn call(
        &self,
        token: Option<&str>,
        method: &str,
        uri: &str,
        extra: &[(&str, &str)],
        body: Vec<u8>,
    ) -> (StatusCode, String) {
        let mut b = Request::builder().method(method).uri(uri);
        if let Some(t) = token {
            b = b.header(TOKEN_HEADER, t);
        }
        for (k, v) in extra {
            b = b.header(*k, *v);
        }
        let res = router(self.app.clone())
            .oneshot(b.body(Body::from(body)).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 22)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    async fn json(&self, token: &str, method: &str, uri: &str, body: &str) -> (StatusCode, String) {
        self.call(
            Some(token),
            method,
            uri,
            &[("content-type", "application/json")],
            body.as_bytes().to_vec(),
        )
        .await
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.dir.join("log")).unwrap_or_default()
    }

    /// The image exists, as far as the fake docker is concerned.
    fn have_image(&self, reference: &str) {
        let name = format!("img_{}", reference.replace(['/', ':'], "_"));
        std::fs::write(self.dir.join(name), "").unwrap();
    }

    fn allowlist(&self, tenant: &str, tag: &str, memory: u32) -> String {
        format!(
            "[images.lab]\nimage = \"necropsy-range/{tenant}/lab:{tag}\"\nuid = {}\nmemory_mb = {memory}\n",
            test_uid()
        )
    }

    /// Pushes an allowlist (and has its image), as the site's deploy does.
    async fn push(
        &self,
        token: &str,
        tenant: &str,
        tag: &str,
        memory: u32,
    ) -> (StatusCode, String) {
        self.have_image(&format!("necropsy-range/{tenant}/lab:{tag}"));
        self.call(
            Some(token),
            "PUT",
            "/v1/allowlist",
            &[],
            self.allowlist(tenant, tag, memory).into_bytes(),
        )
        .await
    }

    async fn make(&self, token: &str, id: &str) -> (StatusCode, String) {
        self.json(
            token,
            "POST",
            "/v1/instances",
            &format!(r#"{{"id":"{id}","image":"lab","ttl_secs":600,"env":{{"NECROPSY_FLAG":"kit{{abc}}"}}}}"#),
        )
        .await
    }
}

// ----------------------------------------------------------------------------- auth

#[tokio::test]
async fn every_request_needs_a_tenants_token_except_the_health_check() {
    let r = rig();
    assert_eq!(
        r.call(None, "GET", "/healthz", &[], vec![]).await.0,
        StatusCode::OK
    );
    for (why, token) in [
        ("no token", None),
        ("a wrong token", Some("not-a-token-0123456789abcdef")),
        ("a token that is too short", Some("short")),
        (
            "the hash of a token, not the token",
            Some(token_hash(PROD_TOKEN).leak() as &str),
        ),
    ] {
        let (s, _) = r.call(token, "GET", "/v1/instances", &[], vec![]).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "{why}");
    }
    assert_eq!(
        r.call(Some(PROD_TOKEN), "GET", "/v1/instances", &[], vec![])
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        r.call(Some(DEV_TOKEN), "GET", "/v1/instances", &[], vec![])
            .await
            .0,
        StatusCode::OK
    );
    // Lab traffic and the control API both need it.
    assert_eq!(
        r.call(None, "GET", "/", &[(INSTANCE_HEADER, "abc")], vec![])
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(r.log(), "", "docker was never run");
}

// ----------------------------------------------------------------------------- allowlist

#[tokio::test]
async fn an_allowlist_must_be_the_tenants_own_and_loaded() {
    let r = rig();
    // An image of the other tenant, or of no tenant: refused whatever exists.
    for image in [
        "necropsy-range/dev/lab:t1",
        "alpine:latest",
        "necropsy-range/prod/../dev/lab:t",
        "necropsy-range/prod/lab",
    ] {
        r.have_image(image);
        let text = format!("[images.lab]\nimage = \"{image}\"\nuid = {}\n", test_uid());
        let (s, why) = r
            .call(
                Some(PROD_TOKEN),
                "PUT",
                "/v1/allowlist",
                &[],
                text.into_bytes(),
            )
            .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{image}: {why}");
    }
    // Its own, but not pushed yet.
    let (s, why) = r
        .call(
            Some(PROD_TOKEN),
            "PUT",
            "/v1/allowlist",
            &[],
            r.allowlist("prod", "notyet", 128).into_bytes(),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{why}");
    assert!(why.contains("push it first"), "{why}");
    // Limits and syntax.
    r.have_image("necropsy-range/prod/lab:t1");
    for (what, text) in [
        ("uid 0", "[images.lab]\nimage = \"necropsy-range/prod/lab:t1\"\nuid = 0\n".to_owned()),
        ("too much memory", format!("[images.lab]\nimage = \"necropsy-range/prod/lab:t1\"\nuid = {}\nmemory_mb = 99999\n", test_uid())),
        ("an unknown key", "[images.lab]\nimage = \"necropsy-range/prod/lab:t1\"\nprivileged = true\n".to_owned()),
        ("a bad lab name", "[images.Lab_1]\nimage = \"necropsy-range/prod/lab:t1\"\n".to_owned()),
        ("not toml", "this is not toml".to_owned()),
    ] {
        let (s, _) = r.call(Some(PROD_TOKEN), "PUT", "/v1/allowlist", &[], text.into_bytes()).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{what}");
    }
    // None of that changed anything.
    assert_eq!(
        r.call(Some(PROD_TOKEN), "GET", "/v1/allowlist", &[], vec![])
            .await
            .1,
        ""
    );
    assert_eq!(
        r.make(PROD_TOKEN, "aaaaaa").await.0,
        StatusCode::BAD_REQUEST
    );

    let (s, why) = r.push(PROD_TOKEN, "prod", "t1", 128).await;
    assert_eq!(s, StatusCode::OK, "{why}");
    assert_eq!(r.make(PROD_TOKEN, "aaaaaa").await.0, StatusCode::CREATED);
    // Tenants have separate allowlists: dev has none.
    assert_eq!(r.make(DEV_TOKEN, "aaaaaa").await.0, StatusCode::BAD_REQUEST);
    assert!(r
        .call(Some(DEV_TOKEN), "GET", "/v1/allowlist", &[], vec![])
        .await
        .1
        .is_empty());
}

#[tokio::test]
async fn allowlists_survive_a_restart_and_old_images_are_pruned() {
    let r = rig();
    assert_eq!(
        r.push(PROD_TOKEN, "prod", "t1", 128).await.0,
        StatusCode::OK
    );
    assert_eq!(
        r.push(PROD_TOKEN, "prod", "t2", 128).await.0,
        StatusCode::OK
    );
    // Docker lists three images; only the current and previous allowlists' stay.
    std::fs::write(
        r.dir.join("imgls"),
        "necropsy-range/prod/lab:t1\nnecropsy-range/prod/lab:t2\nnecropsy-range/prod/lab:old\n",
    )
    .unwrap();
    assert_eq!(
        r.push(PROD_TOKEN, "prod", "t2", 128).await.0,
        StatusCode::OK
    );
    let log = r.log();
    assert!(log.contains("rmi necropsy-range/prod/lab:old"), "{log}");
    assert!(!log.contains("rmi necropsy-range/prod/lab:t2"), "{log}");
    assert!(
        log.contains("--filter reference=necropsy-range/prod/*"),
        "only its own: {log}"
    );

    // A new process reads the allowlist from disk.
    let again = App::new(r.app.cfg.clone(), Docker::new(r.app.cfg.docker.clone()));
    again.recover().await;
    assert!(again.allowlist_of("prod").contains_key("lab"));
    assert!(again.allowlist_of("dev").is_empty());
}

// ----------------------------------------------------------------------------- labs

#[tokio::test]
async fn a_lab_is_started_locked_down_and_stopped() {
    let r = rig();
    r.push(PROD_TOKEN, "prod", "t1", 128).await;
    let (s, b) = r.make(PROD_TOKEN, "abcde12345").await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    assert!(r.dir.join("range/prod/abcde12345/app.sock").exists());

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
        "--name=nr-prod-abcde12345",
        "--label=necropsy.tenant=prod",
        "noexec",
    ] {
        assert!(log.contains(must), "missing {must}: {log}");
    }
    assert!(
        log.trim_end().ends_with("necropsy-range/prod/lab:t1"),
        "the image comes last: {log}"
    );
    assert!(!log.contains("--privileged") && !log.contains("docker.sock"));
    // The flag is a file the lab reads once, never an environment variable: with a
    // supervisor at PID 1, the environment would hand it to anyone with a shell.
    assert!(
        !log.contains("NECROPSY_FLAG") && !log.contains("kit{abc}"),
        "{log}"
    );
    let flag = r.dir.join("range/prod/abcde12345/flag");
    assert_eq!(std::fs::read_to_string(&flag).unwrap(), "kit{abc}");
    use std::os::unix::fs::PermissionsExt as _;
    assert_eq!(
        std::fs::metadata(&flag).unwrap().permissions().mode() & 0o777,
        0o400
    );
    assert!(
        log.contains("--env=NECROPSY_INSTANCE") || !log.contains("NECROPSY_INSTANCE"),
        "{log}"
    );

    assert_eq!(
        r.make(PROD_TOKEN, "abcde12345").await.0,
        StatusCode::CONFLICT
    );
    assert!(r
        .call(Some(PROD_TOKEN), "GET", "/v1/instances", &[], vec![])
        .await
        .1
        .contains("abcde12345"));
    assert_eq!(
        r.json(
            PROD_TOKEN,
            "POST",
            "/v1/instances/abcde12345/touch",
            r#"{"ttl_secs":3000}"#
        )
        .await
        .0,
        StatusCode::OK
    );
    assert!(r.app.lock()[&("prod".to_owned(), "abcde12345".to_owned())].expires > now() + 2900);
    assert_eq!(
        r.json(
            PROD_TOKEN,
            "POST",
            "/v1/instances/nope/touch",
            r#"{"ttl_secs":5}"#
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        r.call(
            Some(PROD_TOKEN),
            "DELETE",
            "/v1/instances/abcde12345",
            &[],
            vec![]
        )
        .await
        .0,
        StatusCode::OK
    );
    assert!(!r.dir.join("range/prod/abcde12345").exists());
    assert!(r.log().contains("rm -f nr-prod-abcde12345"));
    assert!(r.app.lock().is_empty());
}

#[tokio::test]
async fn tenants_cannot_see_or_touch_each_others_labs() {
    let r = rig();
    r.push(PROD_TOKEN, "prod", "t1", 128).await;
    r.push(DEV_TOKEN, "dev", "t1", 128).await;
    assert_eq!(r.make(PROD_TOKEN, "samelabel").await.0, StatusCode::CREATED);
    // The same id is free for the other tenant: they are separate namespaces.
    assert_eq!(r.make(DEV_TOKEN, "samelabel").await.0, StatusCode::CREATED);
    assert!(
        r.dir.join("range/prod/samelabel").exists() && r.dir.join("range/dev/samelabel").exists()
    );
    assert!(
        r.log().contains("--name=nr-dev-samelabel") && r.log().contains("--name=nr-prod-samelabel")
    );

    r.make(PROD_TOKEN, "prodonly01").await;
    assert!(!r
        .call(Some(DEV_TOKEN), "GET", "/v1/instances", &[], vec![])
        .await
        .1
        .contains("prodonly01"));
    // Dev stopping or extending an id that only prod has changes nothing of prod's.
    r.call(
        Some(DEV_TOKEN),
        "DELETE",
        "/v1/instances/prodonly01",
        &[],
        vec![],
    )
    .await;
    assert_eq!(
        r.json(
            DEV_TOKEN,
            "POST",
            "/v1/instances/prodonly01/touch",
            r#"{"ttl_secs":5}"#
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert!(r
        .app
        .lock()
        .contains_key(&("prod".to_owned(), "prodonly01".to_owned())));
    assert!(r.dir.join("range/prod/prodonly01").exists());
}

#[tokio::test]
async fn each_tenant_has_a_lab_count_and_a_memory_budget() {
    let r = rig();
    // prod: 3 labs, 300 MB. Three labs of 100 MB fit; a fourth does not.
    r.push(PROD_TOKEN, "prod", "t1", 100).await;
    for i in 0..3 {
        assert_eq!(
            r.make(PROD_TOKEN, &format!("p{i}aaaa")).await.0,
            StatusCode::CREATED
        );
    }
    assert_eq!(
        r.make(PROD_TOKEN, "p9aaaa").await.0,
        StatusCode::TOO_MANY_REQUESTS
    );
    // dev: 200 MB. Two of 128 MB do not fit in it, though dev's count would allow two.
    r.push(DEV_TOKEN, "dev", "t1", 128).await;
    assert_eq!(r.make(DEV_TOKEN, "d0aaaa").await.0, StatusCode::CREATED);
    let (s, why) = r.make(DEV_TOKEN, "d1aaaa").await;
    assert_eq!(
        (s, why.as_str()),
        (StatusCode::TOO_MANY_REQUESTS, "the range is full")
    );
    // dev being full leaves prod's room alone, and the other way round.
    r.call(
        Some(PROD_TOKEN),
        "DELETE",
        "/v1/instances/p0aaaa",
        &[],
        vec![],
    )
    .await;
    assert_eq!(r.make(PROD_TOKEN, "p5aaaa").await.0, StatusCode::CREATED);
    // A smaller lab fits in what dev has left.
    assert_eq!(r.push(DEV_TOKEN, "dev", "t2", 64).await.0, StatusCode::OK);
    assert_eq!(r.make(DEV_TOKEN, "d2aaaa").await.0, StatusCode::CREATED);
}

#[tokio::test]
async fn bad_requests_are_refused_before_docker_is_run_and_failures_clean_up() {
    let r = rig();
    r.push(PROD_TOKEN, "prod", "t1", 128).await;
    let before = r.log().lines().count();
    for (why, body) in [
        (
            "unknown lab",
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
        assert_eq!(
            r.json(PROD_TOKEN, "POST", "/v1/instances", body).await.0,
            StatusCode::BAD_REQUEST,
            "{why}"
        );
    }
    assert_eq!(
        r.log().lines().count(),
        before,
        "docker was not run for them"
    );
    assert_eq!(
        r.call(
            Some(PROD_TOKEN),
            "DELETE",
            "/v1/instances/Bad..id",
            &[],
            vec![]
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );

    let (s, _) = r.make(PROD_TOKEN, "failme1").await;
    assert_eq!(s, StatusCode::BAD_GATEWAY);
    assert!(!r.dir.join("range/prod/failme1").exists());
    assert!(r.app.lock().is_empty(), "a failed start frees its slot");
}

#[tokio::test]
async fn expired_labs_are_reaped_and_survivors_recovered() {
    let r = rig();
    let t = now();
    std::fs::write(
        r.dir.join("ps"),
        format!(
            "nr-prod-old1 prod lab 128 {}\nnr-prod-live1 prod lab 128 {}\nnr-dev-live2 dev lab 128 {}\nnr-ghost-x ghost lab 128 {}\nstranger x y 1 2\n",
            t - 5, t + 500, t + 500, t + 500
        ),
    )
    .unwrap();
    for d in ["prod/old1", "prod/live1", "dev/live2", "prod/orphan"] {
        std::fs::create_dir_all(r.dir.join("range").join(d)).unwrap();
    }
    r.app.recover().await;
    assert!(r.log().contains("rm -f nr-prod-old1"));
    {
        let live = r.app.lock();
        assert!(live.contains_key(&("prod".into(), "live1".into())));
        assert!(live.contains_key(&("dev".into(), "live2".into())));
        assert!(!live.contains_key(&("prod".into(), "old1".into())));
        assert!(
            !live.keys().any(|(t, _)| t == "ghost"),
            "an unknown tenant's container is left alone"
        );
        assert_eq!(
            live[&("prod".into(), "live1".into())].memory_mb,
            128,
            "memory is recovered for the budget"
        );
    }
    assert!(r.dir.join("range/prod/live1").exists() && r.dir.join("range/dev/live2").exists());
    assert!(!r.dir.join("range/prod/old1").exists());
    assert!(
        !r.dir.join("range/prod/orphan").exists(),
        "an unowned directory is removed"
    );

    r.app
        .lock()
        .get_mut(&("prod".into(), "live1".into()))
        .unwrap()
        .expires = t - 1;
    r.app.reap().await;
    assert!(!r.app.lock().contains_key(&("prod".into(), "live1".into())));
    assert!(
        r.app.lock().contains_key(&("dev".into(), "live2".into())),
        "reaping one tenant's lab leaves the other's"
    );
}

// ----------------------------------------------------------------------------- images

fn archive(manifest: Option<&str>) -> Vec<u8> {
    let mut b = tar::Builder::new(Vec::new());
    // Blobs come first in a real archive: the manifest is found by scanning.
    for (name, data) in [
        ("blobs/sha256/aa", b"layer".as_slice()),
        ("oci-layout", b"{}"),
    ] {
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        b.append_data(&mut h, name, data).unwrap();
    }
    if let Some(m) = manifest {
        let mut h = tar::Header::new_gnu();
        h.set_size(m.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        b.append_data(&mut h, "manifest.json", m.as_bytes())
            .unwrap();
    }
    b.into_inner().unwrap()
}

#[tokio::test]
async fn an_uploaded_image_must_carry_only_the_tenants_own_tags() {
    let r = rig();
    let good = r#"[{"Config":"c","RepoTags":["necropsy-range/prod/lab:abc123"],"Layers":["l"]}]"#;
    let (s, why) = r
        .call(
            Some(PROD_TOKEN),
            "POST",
            "/v1/images",
            &[],
            archive(Some(good)),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{why}");
    assert_eq!(why, "necropsy-range/prod/lab:abc123");
    assert!(r.log().contains("load -i"), "{}", r.log());
    assert!(
        std::fs::read_dir(r.dir.join("state/uploads"))
            .unwrap()
            .next()
            .is_none(),
        "the spooled upload is removed"
    );

    let loads = || r.log().matches("load -i").count();
    let before = loads();
    for (why, body) in [
        (
            "another tenant's tag",
            archive(Some(r#"[{"RepoTags":["necropsy-range/dev/lab:abc"]}]"#)),
        ),
        (
            "one good tag and one not",
            archive(Some(
                r#"[{"RepoTags":["necropsy-range/prod/lab:a","necropsy-range/dev/lab:a"]}]"#,
            )),
        ),
        (
            "a tag of an unrelated image",
            archive(Some(r#"[{"RepoTags":["alpine:latest"]}]"#)),
        ),
        (
            "a path trick",
            archive(Some(
                r#"[{"RepoTags":["necropsy-range/prod/../dev/lab:a"]}]"#,
            )),
        ),
        ("no tag at all", archive(Some(r#"[{"RepoTags":[]}]"#))),
        ("a null tag list", archive(Some(r#"[{"RepoTags":null}]"#))),
        ("no images", archive(Some("[]"))),
        ("a manifest that is not json", archive(Some("nope"))),
        ("no manifest", archive(None)),
        ("not a tar file", b"this is not an archive".to_vec()),
        ("nothing", vec![]),
    ] {
        let (s, msg) = r
            .call(Some(PROD_TOKEN), "POST", "/v1/images", &[], body)
            .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{why}: {msg}");
    }
    assert_eq!(loads(), before, "none of those reached docker load");
    assert!(std::fs::read_dir(r.dir.join("state/uploads"))
        .unwrap()
        .next()
        .is_none());

    // The same archive is fine for dev only if it carries dev's tags.
    let (s, _) = r
        .call(
            Some(DEV_TOKEN),
            "POST",
            "/v1/images",
            &[],
            archive(Some(good)),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // A docker that cannot load it is a bad gateway, with the spool cleaned up.
    std::fs::write(r.dir.join("load_fail"), "").unwrap();
    let (s, _) = r
        .call(
            Some(PROD_TOKEN),
            "POST",
            "/v1/images",
            &[],
            archive(Some(good)),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_GATEWAY);
    assert!(std::fs::read_dir(r.dir.join("state/uploads"))
        .unwrap()
        .next()
        .is_none());
}

// ----------------------------------------------------------------------------- lab traffic

#[tokio::test]
async fn lab_traffic_reaches_the_labs_own_socket_and_only_the_tenants_own() {
    let r = rig();
    r.push(PROD_TOKEN, "prod", "t1", 128).await;
    r.push(DEV_TOKEN, "dev", "t1", 128).await;
    assert_eq!(
        r.make(PROD_TOKEN, "lab1aaaaaa").await.0,
        StatusCode::CREATED
    );

    // The "lab": a web server on the socket, which echoes what it receives.
    let sock = r.dir.join("range/prod/lab1aaaaaa/app.sock");
    std::fs::remove_file(&sock).unwrap();
    let listener = UnixListener::bind(&sock).unwrap();
    let lab = Router::new().fallback(axum::routing::any(
        |req: axum::extract::Request| async move {
            let (parts, body) = req.into_parts();
            let body = axum::body::to_bytes(body, 1 << 20).await.unwrap();
            let mut seen: Vec<String> = parts
                .headers
                .keys()
                .map(|k| k.as_str().to_owned())
                .filter(|k| k.starts_with("x-"))
                .collect();
            seen.sort();
            (
                [("x-from-lab", "yes"), ("connection", "keep-alive")],
                format!(
                    "{} {} body={} auth={} headers={}",
                    parts.method,
                    parts.uri,
                    String::from_utf8_lossy(&body),
                    parts
                        .headers
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("-"),
                    seen.join(",")
                ),
            )
        },
    ));
    tokio::spawn(async move {
        let _ = axum::serve(listener, lab).await;
    });

    let (s, body) = r
        .call(
            Some(PROD_TOKEN),
            "POST",
            "/some/path?x=1",
            &[
                (INSTANCE_HEADER, "lab1aaaaaa"),
                ("authorization", "Basic cGxheWVy"),
                ("x-player-header", "hello"),
                ("x-necropsy-extra", "smuggled"),
            ],
            b"q=id".to_vec(),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert!(body.starts_with("POST /some/path?x=1 body=q=id"), "{body}");
    // The player's own Authorization header reaches the lab, ours never does.
    assert!(body.contains("auth=Basic cGxheWVy"), "{body}");
    assert!(
        body.contains("headers=x-player-header") && !body.contains("x-necropsy"),
        "{body}"
    );

    // Another tenant cannot reach it, nor an id that does not exist.
    let (s, _) = r
        .call(
            Some(DEV_TOKEN),
            "GET",
            "/",
            &[(INSTANCE_HEADER, "lab1aaaaaa")],
            vec![],
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = r
        .call(
            Some(PROD_TOKEN),
            "GET",
            "/",
            &[(INSTANCE_HEADER, "nosuchlab1")],
            vec![],
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = r
        .call(
            Some(PROD_TOKEN),
            "GET",
            "/",
            &[(INSTANCE_HEADER, "../../etc")],
            vec![],
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    // The header is what makes it lab traffic: without it a path is the control API.
    let (s, _) = r
        .call(Some(PROD_TOKEN), "GET", "/some/path", &[], vec![])
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // A lab that has gone away is a bad gateway, not a hang.
    std::fs::remove_file(&sock).unwrap();
    let (s, _) = r
        .call(
            Some(PROD_TOKEN),
            "GET",
            "/",
            &[(INSTANCE_HEADER, "lab1aaaaaa")],
            vec![],
        )
        .await;
    assert_eq!(s, StatusCode::BAD_GATEWAY);
}

// ----------------------------------------------------------------------------- self-test

/// A lab that answers on its socket, as `selftest` needs.
async fn answering_lab(r: &Rig, tenant: &str) {
    // The fake docker `touch`es the socket; here a server must be listening. The
    // self-test creates the lab itself, so serve every id that appears.
    let range = r.dir.join("range").join(tenant);
    tokio::spawn(async move {
        let mut served = std::collections::HashSet::new();
        for _ in 0..200 {
            if let Ok(rd) = std::fs::read_dir(&range) {
                for e in rd.flatten() {
                    let sock = e.path().join("app.sock");
                    if sock.exists() && served.insert(sock.clone()) {
                        let _ = std::fs::remove_file(&sock);
                        if let Ok(l) = UnixListener::bind(&sock) {
                            let app = Router::new().fallback(|| async { "hello" });
                            tokio::spawn(async move {
                                let _ = axum::serve(l, app).await;
                            });
                        }
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
}

#[tokio::test]
async fn the_selftest_starts_every_lab_checks_it_and_cleans_up() {
    let r = rig();
    assert_eq!(
        r.call(Some(PROD_TOKEN), "POST", "/v1/selftest", &[], vec![])
            .await
            .0,
        StatusCode::BAD_REQUEST,
        "no labs yet"
    );
    r.push(PROD_TOKEN, "prod", "t1", 128).await;
    answering_lab(&r, "prod").await;
    let (s, report) = r
        .call(Some(PROD_TOKEN), "POST", "/v1/selftest", &[], vec![])
        .await;
    assert_eq!(s, StatusCode::OK, "{report}");
    for part in [
        "answers on its socket",
        "locked down",
        "no network interface but loopback",
        "runs as uid 10001",
        "all 1 lab(s) passed",
    ] {
        assert!(report.contains(part), "{part}: {report}");
    }
    assert!(r.app.lock().is_empty(), "the test lab is gone");
    assert!(r.log().contains("rm -f nr-prod-st"), "{}", r.log());
    assert_eq!(
        std::fs::read_dir(r.dir.join("range/prod")).unwrap().count(),
        0
    );
}

#[tokio::test]
async fn the_selftest_fails_a_lab_that_breaks_a_promise() {
    let r = rig();
    r.push(PROD_TOKEN, "prod", "t1", 128).await;
    answering_lab(&r, "prod").await;
    for (what, from, to) in [
        (
            "a network",
            r#""NetworkMode":"none""#,
            r#""NetworkMode":"bridge""#,
        ),
        (
            "a writable root",
            r#""ReadonlyRootfs":true"#,
            r#""ReadonlyRootfs":false"#,
        ),
        (
            "privileged",
            r#""Privileged":false"#,
            r#""Privileged":true"#,
        ),
        ("capabilities", r#""CapDrop":["ALL"]"#, r#""CapDrop":[]"#),
        (
            "no-new-privileges",
            "no-new-privileges:true",
            "seccomp=unconfined",
        ),
        ("swap", r#""MemorySwap":134217728"#, r#""MemorySwap":-1"#),
        ("no cpu limit", r#""NanoCpus":500000000"#, r#""NanoCpus":0"#),
        ("no process limit", r#""PidsLimit":64"#, r#""PidsLimit":0"#),
        ("root", r#""User":"10001:10001""#, r#""User":"""#),
        (
            "the docker socket",
            r#""Source":"/run/x""#,
            r#""Source":"/var/run/docker.sock""#,
        ),
    ] {
        std::fs::write(r.dir.join("inspect.json"), GOOD_INSPECT.replace(from, to)).unwrap();
        let (s, report) = r
            .call(Some(PROD_TOKEN), "POST", "/v1/selftest", &[], vec![])
            .await;
        assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR, "{what}: {report}");
        assert!(report.contains("FAIL"), "{what}: {report}");
        assert!(
            r.app.lock().is_empty(),
            "{what}: the lab is removed even so"
        );
    }
    // An extra network interface seen from inside.
    std::fs::write(r.dir.join("inspect.json"), GOOD_INSPECT).unwrap();
    std::fs::write(r.dir.join("netdev"), format!("{GOOD_NETDEV}  eth0: 1 1\n")).unwrap();
    let (s, report) = r
        .call(Some(PROD_TOKEN), "POST", "/v1/selftest", &[], vec![])
        .await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR, "{report}");
    assert!(report.contains("besides loopback"), "{report}");
}

// ----------------------------------------------------------------------------- config

#[test]
fn tenants_are_checked() {
    let h = token_hash("x");
    for bad in [
        String::new(),
        format!("[tenants.Prod]\ntoken_sha256 = \"{h}\"\nmax_instances = 1\nmemory_budget_mb = 256"),
        format!("[tenants.a-b]\ntoken_sha256 = \"{h}\"\nmax_instances = 1\nmemory_budget_mb = 256"),
        "[tenants.prod]\ntoken_sha256 = \"abc\"\nmax_instances = 1\nmemory_budget_mb = 256".into(),
        format!("[tenants.prod]\ntoken_sha256 = \"{}\"\nmax_instances = 1\nmemory_budget_mb = 256", h.to_uppercase()),
        format!("[tenants.prod]\ntoken_sha256 = \"{h}\"\nmax_instances = 0\nmemory_budget_mb = 256"),
        format!("[tenants.prod]\ntoken_sha256 = \"{h}\"\nmax_instances = 1\nmemory_budget_mb = 8"),
        format!("[tenants.prod]\ntoken_sha256 = \"{h}\"\nmax_instances = 1\nmemory_budget_mb = 256\nsurprise = 1"),
    ] {
        assert!(config::parse_tenants(&bad).is_err(), "{bad}");
    }
    let ok: BTreeMap<_, _> = config::parse_tenants(&format!(
        "[tenants.prod]\ntoken_sha256 = \"{h}\"\nmax_instances = 5\nmemory_budget_mb = 1024"
    ))
    .unwrap();
    assert_eq!(ok["prod"].max_instances, 5);
}

/// Every container-backed pack names a lab that exists in `range/`, and the
/// allowlist template parses for a tenant.
#[test]
fn packs_only_name_labs_the_repository_has() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let text = std::fs::read_to_string(root.join("range/images.toml")).unwrap();
    // The repository's file is a template: its tags are rewritten per tenant on deploy.
    let rewritten = text
        .replace(":1\"", ":abc123\"")
        .replace("necropsy-range/", "necropsy-range/prod/");
    let images = config::parse_images(&rewritten, "prod").unwrap();
    for p in necropsy_pack::load_all(&root.join("packs")).unwrap() {
        if let Some(c) = p.live.as_ref().and_then(|l| l.container.as_ref()) {
            assert!(images.contains_key(&c.image), "{}: {}", p.meta.id, c.image);
            assert!(
                root.join("range")
                    .join(&c.image)
                    .join("Dockerfile")
                    .exists(),
                "{}",
                c.image
            );
        }
    }
}

/// Parses the tenants file named by `SPAWNER_CHECK_TENANTS`, if set: the installer's
/// test runs this to check that what `install-range.sh` writes is what this reads.
#[test]
fn generated_tenants_file() {
    let Ok(path) = std::env::var("SPAWNER_CHECK_TENANTS") else {
        return;
    };
    let text = std::fs::read_to_string(&path).unwrap();
    let tenants = config::parse_tenants(&text).unwrap_or_else(|e| panic!("{path}: {e}"));
    assert!(!tenants.is_empty());
}
