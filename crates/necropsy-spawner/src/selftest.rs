// SPDX-License-Identifier: AGPL-3.0-or-later
//! The lab self-test. Starts each of a tenant's labs, once, the way a player's
//! would be started, and checks it keeps the range's promises: it answers HTTP
//! on its socket; Docker has it with no network, a read-only root, no
//! capabilities, no new privileges, not privileged, an unprivileged user and hard
//! limits; and, with what the image has, it has no interface but loopback, is not
//! uid 0 and cannot write its root. Then it stops it and checks nothing is left.
//!
//! The site's deploy asks for this before it moves any traffic, so a lab that
//! cannot be started safely never reaches a player.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Bytes;
use http_body_util::Full;
use serde_json::Value;

use crate::App;

/// Docker's view of a lab, against what the range promises.
pub fn check_inspect(v: &Value) -> Result<(), String> {
    let host = &v["HostConfig"];
    if host["NetworkMode"].as_str() != Some("none") {
        return Err("it has a network".into());
    }
    if host["ReadonlyRootfs"].as_bool() != Some(true) {
        return Err("the root filesystem is writable".into());
    }
    if host["Privileged"].as_bool() != Some(false) {
        return Err("it is privileged".into());
    }
    let drops = host["CapDrop"].as_array().cloned().unwrap_or_default();
    if !drops
        .iter()
        .any(|c| c.as_str().is_some_and(|c| c.eq_ignore_ascii_case("all")))
    {
        return Err("capabilities are not dropped".into());
    }
    let sec = host["SecurityOpt"].as_array().cloned().unwrap_or_default();
    if !sec
        .iter()
        .any(|s| s.as_str().is_some_and(|s| s.contains("no-new-privileges")))
    {
        return Err("no-new-privileges is not set".into());
    }
    let memory = host["Memory"].as_i64().unwrap_or(0);
    if memory <= 0 {
        return Err("there is no memory limit".into());
    }
    if host["MemorySwap"].as_i64() != Some(memory) {
        return Err("it can use swap".into());
    }
    if host["NanoCpus"].as_i64().unwrap_or(0) <= 0 {
        return Err("there is no cpu limit".into());
    }
    if host["PidsLimit"].as_i64().unwrap_or(0) <= 0 {
        return Err("there is no process limit".into());
    }
    let user = v["Config"]["User"].as_str().unwrap_or("");
    if matches!(user, "" | "0" | "root") || user.starts_with("0:") || user.starts_with("root:") {
        return Err(format!("it runs as root ({user:?})"));
    }
    let mounts = v["Mounts"].as_array().cloned().unwrap_or_default();
    if mounts.iter().any(|m| {
        m["Source"]
            .as_str()
            .is_some_and(|s| s.contains("docker.sock"))
    }) {
        return Err("it has the Docker socket".into());
    }
    Ok(())
}

fn random_id() -> String {
    use std::io::Read;
    let mut b = [0u8; 5];
    let _ = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b));
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("st{hex}")
}

/// Tests one lab. Returns what passed, or the first thing that did not.
pub async fn check_lab(app: &Arc<App>, tenant: &str, lab: &str) -> Result<Vec<String>, String> {
    let id = random_id();
    let mut env = HashMap::new();
    env.insert("NECROPSY_FLAG".to_owned(), "selftest{ok}".to_owned());
    env.insert("NECROPSY_INSTANCE".to_owned(), id.clone());
    app.create(
        tenant,
        crate::CreateReq {
            id: id.clone(),
            image: lab.to_owned(),
            ttl_secs: 180,
            env,
        },
    )
    .await
    .map_err(|(s, why)| format!("{lab}: could not start it ({s}: {why})"))?;

    let result = run_checks(app, tenant, lab, &id).await;
    app.teardown(tenant, &id).await;
    let result = result?;

    let name = crate::container_name(tenant, &id);
    if app.docker.run(&["inspect", &name]).await.is_ok() {
        return Err(format!("{lab}: its container was not removed"));
    }
    if app.dir(tenant, &id).exists() {
        return Err(format!("{lab}: its socket directory was not removed"));
    }
    Ok(result)
}

async fn run_checks(
    app: &Arc<App>,
    tenant: &str,
    lab: &str,
    id: &str,
) -> Result<Vec<String>, String> {
    let mut ok = Vec::new();
    let name = crate::container_name(tenant, id);

    // It speaks HTTP on its socket. Any status will do. The socket appears as the
    // lab starts, before what is behind it (a web server the socket bridges to) is
    // listening, so ask for a few seconds before giving up.
    let socket = app.dir(tenant, id).join("app.sock");
    let mut last = String::new();
    let mut answered = false;
    for _ in 0..100 {
        let req = hyper::Request::builder()
            .uri("/")
            .header("host", "lab")
            .header("connection", "close")
            .body(Full::new(Bytes::new()))
            .map_err(|e| e.to_string())?;
        match crate::uds::send(&socket, req, 1024 * 1024).await {
            Ok(_) => {
                answered = true;
                break;
            }
            Err(e) => last = e,
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    if !answered {
        return Err(format!(
            "{lab}: it does not answer HTTP on its socket ({last})"
        ));
    }
    ok.push(format!("{lab}: answers on its socket"));

    let inspect = app
        .docker
        .inspect(&name)
        .await
        .map_err(|e| format!("{lab}: cannot inspect it ({e})"))?;
    check_inspect(&inspect).map_err(|e| format!("{lab}: {e}"))?;
    ok.push(format!("{lab}: locked down in Docker's settings"));

    // Inside it, with whatever the image has.
    if let Some((true, out)) = app.docker.exec(&name, &["cat", "/proc/net/dev"]).await {
        let extra = out
            .lines()
            .skip(2)
            .filter_map(|l| l.split(':').next())
            .map(str::trim)
            .any(|i| !i.is_empty() && i != "lo");
        if extra {
            return Err(format!(
                "{lab}: it has a network interface besides loopback"
            ));
        }
        ok.push(format!("{lab}: no network interface but loopback"));
    }
    if let Some((true, out)) = app.docker.exec(&name, &["sh", "-c", "id -u"]).await {
        if out.trim() == "0" {
            return Err(format!("{lab}: it is running as uid 0"));
        }
        ok.push(format!("{lab}: runs as uid {}", out.trim()));
    }
    if let Some((true, _)) = app
        .docker
        .exec(&name, &["sh", "-c", "touch /selftest-write 2>/dev/null"])
        .await
    {
        return Err(format!("{lab}: it could write to its root filesystem"));
    }
    Ok(ok)
}
