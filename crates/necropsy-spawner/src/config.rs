// SPDX-License-Identifier: AGPL-3.0-or-later
//! Settings: a few environment variables, the tenants, and each tenant's lab
//! allowlist.
//!
//! The spawner serves several tenants, one per site (production, dev). Each has
//! its own token, its own labs and its own limits, so one cannot see, stop or
//! starve another.
//!
//! ```toml
//! # /etc/necropsy-range/tenants.toml
//! [tenants.prod]
//! token_sha256 = "<sha256 of the token, hex>"
//! max_instances = 40          # labs running at once
//! memory_budget_mb = 6000     # the most memory its running labs may be limited to, together
//!
//! [tenants.dev]
//! token_sha256 = "..."
//! max_instances = 15
//! memory_budget_mb = 2500
//! ```
//!
//! A tenant's allowlist is pushed by its site on each deploy and kept in the
//! state directory:
//!
//! ```toml
//! [images.webshell-lab]
//! image = "necropsy-range/prod/webshell-lab:3f2a9c1e5b2d"
//! memory_mb = 128      # hard limit, swap included; the tmpfs counts toward it
//! cpus = 0.5
//! pids = 64
//! tmpfs_mb = 16        # the only writable place in the container
//! uid = 10001          # the user the image runs as
//! ```

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Image {
    pub image: String,
    #[serde(default = "d_memory")]
    pub memory_mb: u32,
    #[serde(default = "d_cpus")]
    pub cpus: f32,
    #[serde(default = "d_pids")]
    pub pids: u32,
    #[serde(default = "d_tmpfs")]
    pub tmpfs_mb: u32,
    #[serde(default = "d_uid")]
    pub uid: u32,
    /// Allow running programs from the writable tmpfs. Off by default.
    #[serde(default)]
    pub tmpfs_exec: bool,
}

fn d_memory() -> u32 {
    128
}
fn d_cpus() -> f32 {
    0.5
}
fn d_pids() -> u32 {
    64
}
fn d_tmpfs() -> u32 {
    16
}
fn d_uid() -> u32 {
    10001
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImagesFile {
    images: BTreeMap<String, Image>,
}

/// Most labs one allowlist may name.
pub const MAX_IMAGES: usize = 32;

/// Parses a tenant's allowlist. `tenant` is who is pushing it: every image must
/// live under `necropsy-range/<tenant>/`, so one tenant can never name, and so
/// never run, another's.
pub fn parse_images(text: &str, tenant: &str) -> Result<BTreeMap<String, Image>, String> {
    let f: ImagesFile = toml::from_str(text).map_err(|e| e.to_string())?;
    if f.images.len() > MAX_IMAGES {
        return Err(format!("at most {MAX_IMAGES} labs"));
    }
    let prefix = format!("necropsy-range/{tenant}/");
    for (name, i) in &f.images {
        let ok_name = !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if !ok_name {
            return Err(format!("bad lab name {name:?}"));
        }
        if !valid_image_ref(&i.image, &prefix) {
            return Err(format!(
                "lab {name:?}: the image must look like {prefix}<lab>:<tag>, not {:?}",
                i.image
            ));
        }
        if !(16..=2048).contains(&i.memory_mb)
            || !(0.05..=4.0).contains(&i.cpus)
            || !(8..=512).contains(&i.pids)
            || !(1..=256).contains(&i.tmpfs_mb)
            || i.tmpfs_mb >= i.memory_mb
            || i.uid == 0
        {
            return Err(format!(
                "limits of {name:?} are out of range (and uid 0 is not allowed)"
            ));
        }
    }
    Ok(f.images)
}

/// `necropsy-range/<tenant>/<lab>:<tag>`, with plain characters only.
pub fn valid_image_ref(r: &str, prefix: &str) -> bool {
    let Some(rest) = r.strip_prefix(prefix) else {
        return false;
    };
    let Some((lab, tag)) = rest.split_once(':') else {
        return false;
    };
    !lab.is_empty()
        && lab
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && (1..=64).contains(&tag.len())
        && tag
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-')
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tenant {
    /// Lower-case hex SHA-256 of the tenant's token.
    pub token_sha256: String,
    pub max_instances: usize,
    pub memory_budget_mb: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TenantsFile {
    tenants: BTreeMap<String, Tenant>,
}

pub fn parse_tenants(text: &str) -> Result<BTreeMap<String, Tenant>, String> {
    let f: TenantsFile = toml::from_str(text).map_err(|e| e.to_string())?;
    if f.tenants.is_empty() {
        return Err("no tenants".into());
    }
    for (name, t) in &f.tenants {
        let ok_name = !name.is_empty()
            && name.len() <= 16
            && name.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
        if !ok_name {
            return Err(format!(
                "tenant name {name:?} must be 1 to 16 lower-case letters and digits, starting with a letter"
            ));
        }
        if t.token_sha256.len() != 64
            || !t
                .token_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(format!(
                "tenant {name:?}: token_sha256 is not 64 hex digits"
            ));
        }
        if !(1..=1000).contains(&t.max_instances)
            || !(128..=1_000_000).contains(&t.memory_budget_mb)
        {
            return Err(format!("tenant {name:?}: limits are out of range"));
        }
    }
    Ok(f.tenants)
}

#[derive(Debug, Clone)]
pub struct Config {
    /// Where the control API and the lab traffic are served, for the site
    /// servers. Put it on the private network, and firewall it to them.
    pub listen: SocketAddr,
    /// Allowlists, and uploaded images while they are checked.
    pub state_dir: PathBuf,
    /// Each lab's socket directory, `<range_dir>/<tenant>/<id>`.
    pub range_dir: PathBuf,
    pub docker: String,
    pub tenants: BTreeMap<String, Tenant>,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let var = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_owned());
        let tenants_path = var("SPAWNER_TENANTS", "/etc/necropsy-range/tenants.toml");
        let text = std::fs::read_to_string(&tenants_path)
            .map_err(|e| format!("cannot read {tenants_path}: {e}"))?;
        let tenants = parse_tenants(&text).map_err(|e| format!("{tenants_path}: {e}"))?;
        let listen = var("SPAWNER_LISTEN", "127.0.0.1:9400");
        Ok(Self {
            listen: listen.parse().map_err(|_| {
                format!("SPAWNER_LISTEN {listen:?} must be host:port, like 10.0.0.50:9400")
            })?,
            state_dir: var("SPAWNER_STATE", "/var/lib/necropsy-range").into(),
            range_dir: var("SPAWNER_RANGE_DIR", "/run/necropsy-range").into(),
            docker: var("SPAWNER_DOCKER", "docker"),
            tenants,
        })
    }
}
