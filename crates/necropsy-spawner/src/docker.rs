// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Docker command line. Arguments are passed as a vector, never through a
//! shell, and every value in them has been checked by the caller.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use tokio::process::Command;

use crate::config::Image;

/// Longest a single docker command may take.
const TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct Docker {
    bin: String,
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

    /// Our containers: `(id, image name, expiry)`.
    pub async fn list(&self) -> Result<Vec<(String, String, i64)>, String> {
        let out = self
            .run(&[
                "ps",
                "-a",
                "--filter",
                "label=necropsy.range=1",
                "--format",
                r#"{{.Names}} {{.Label "necropsy.image"}} {{.Label "necropsy.expires"}}"#,
            ])
            .await?;
        Ok(out
            .lines()
            .filter_map(|l| {
                let mut f = l.split_whitespace();
                let (name, image, expires) = (f.next()?, f.next()?, f.next()?);
                Some((
                    name.strip_prefix("nr-")?.to_owned(),
                    image.to_owned(),
                    expires.parse().ok()?,
                ))
            })
            .collect())
    }
}

/// The `docker run` arguments for an instance. The container gets no network,
/// a read-only root, no capabilities, hard memory, cpu and process limits, and
/// one writable tmpfs. The only door is the socket in `dir`.
pub fn run_args(
    name: &str,
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
    a.push(format!("--label=necropsy.image={image_name}"));
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
