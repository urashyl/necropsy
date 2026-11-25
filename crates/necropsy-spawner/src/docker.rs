// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Docker command line. Arguments are passed as a vector, never through a
//! shell, and every value in them has been checked by the caller.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use tokio::process::Command;

use crate::config::Image;

/// Longest a single docker command may take.
const TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone)]
pub struct Docker {
    bin: String,
}

/// A lab container as Docker lists it.
pub struct Listed {
    pub tenant: String,
    pub id: String,
    pub image: String,
    pub memory_mb: u64,
    pub expires: i64,
}

impl Docker {
    pub fn new(bin: String) -> Self {
        Self { bin }
    }

    /// Runs `docker <args>`, returning stdout, or the error text.
    pub async fn run(&self, args: &[&str]) -> Result<String, String> {
        let out = tokio::time::timeout(
            TIMEOUT,
            Command::new(&self.bin)
                .args(args)
                .stdin(std::process::Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| format!("docker {} timed out", args.first().unwrap_or(&"")))?
        .map_err(|e| format!("cannot run {}: {e}", self.bin))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
        }
    }

    pub async fn image_exists(&self, reference: &str) -> bool {
        self.run(&["image", "inspect", reference]).await.is_ok()
    }

    /// Loads an image archive that has already been checked.
    pub async fn load(&self, path: &Path) -> Result<(), String> {
        self.run(&["load", "-i", &path.display().to_string()])
            .await
            .map(|_| ())
    }

    /// Our containers.
    pub async fn list(&self) -> Result<Vec<Listed>, String> {
        let out = self
            .run(&[
                "ps",
                "-a",
                "--filter",
                "label=necropsy.range=1",
                "--format",
                r#"{{.Names}} {{.Label "necropsy.tenant"}} {{.Label "necropsy.image"}} {{.Label "necropsy.memory"}} {{.Label "necropsy.expires"}}"#,
            ])
            .await?;
        Ok(out
            .lines()
            .filter_map(|l| {
                let mut f = l.split_whitespace();
                let (name, tenant, image, memory, expires) =
                    (f.next()?, f.next()?, f.next()?, f.next()?, f.next()?);
                let id = name.strip_prefix(&format!("nr-{tenant}-"))?;
                Some(Listed {
                    tenant: tenant.to_owned(),
                    id: id.to_owned(),
                    image: image.to_owned(),
                    memory_mb: memory.parse().ok()?,
                    expires: expires.parse().ok()?,
                })
            })
            .collect())
    }

    /// `docker inspect` of one container, as JSON.
    pub async fn inspect(&self, name: &str) -> Result<serde_json::Value, String> {
        let out = self.run(&["inspect", name]).await?;
        let v: serde_json::Value = serde_json::from_str(&out).map_err(|e| e.to_string())?;
        v.get(0).cloned().ok_or_else(|| "empty inspect".into())
    }

    /// Runs a command inside a container. `None` if it could not be run at all
    /// (the image may not have the tool), `Some((success, stdout))` otherwise.
    pub async fn exec(&self, name: &str, cmd: &[&str]) -> Option<(bool, String)> {
        let mut args = vec!["exec", name];
        args.extend_from_slice(cmd);
        let out = tokio::time::timeout(
            Duration::from_secs(20),
            Command::new(&self.bin)
                .args(&args)
                .stdin(std::process::Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .ok()?
        .ok()?;
        // Docker exits 126 or 127 when the program is not in the image.
        match out.status.code() {
            Some(126) | Some(127) => None,
            _ => Some((
                out.status.success(),
                String::from_utf8_lossy(&out.stdout).into_owned(),
            )),
        }
    }
}

/// The `docker run` arguments for an instance. The container gets no network,
/// a read-only root, no capabilities, hard memory, cpu and process limits, and
/// one writable tmpfs. The only door is the socket in `dir`.
#[allow(clippy::too_many_arguments)]
pub fn run_args(
    name: &str,
    tenant: &str,
    image_name: &str,
    img: &Image,
    dir: &Path,
    expires: i64,
    env: &HashMap<String, String>,
) -> Vec<String> {
    let mut a: Vec<String> = [
        "run",
        "--detach",
        "--init",
        "--restart=no",
        "--network=none",
        "--read-only",
        "--cap-drop=ALL",
        "--security-opt=no-new-privileges:true",
        "--stop-timeout=1",
        // Nothing an instance prints is kept: a chatty one cannot fill the disk.
        "--log-driver=none",
        "--ulimit=nofile=256:256",
        "--ulimit=core=0",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    a.push(format!("--name={name}"));
    a.push("--label=necropsy.range=1".into());
    a.push(format!("--label=necropsy.tenant={tenant}"));
    a.push(format!("--label=necropsy.image={image_name}"));
    a.push(format!("--label=necropsy.memory={}", img.memory_mb));
    a.push(format!("--label=necropsy.expires={expires}"));
    a.push(format!("--user={0}:{0}", img.uid));
    a.push(format!("--memory={}m", img.memory_mb));
    a.push(format!("--memory-swap={}m", img.memory_mb));
    a.push(format!("--cpus={}", img.cpus));
    a.push(format!("--pids-limit={}", img.pids));
    a.push(format!(
        "--tmpfs=/tmp:rw,nosuid,{},size={}m,uid={},mode=0700",
        if img.tmpfs_exec { "exec" } else { "noexec" },
        img.tmpfs_mb,
        img.uid
    ));
    a.push(format!("--volume={}:/run/sock", dir.display()));
    let mut keys: Vec<&String> = env.keys().collect();
    keys.sort();
    for k in keys {
        a.push(format!("--env={k}={}", env[k]));
    }
    // After every option: the image, then nothing.
    a.push(img.image.clone());
    a
}
