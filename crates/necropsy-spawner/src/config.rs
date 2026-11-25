// SPDX-License-Identifier: AGPL-3.0-or-later
//! Settings: a few environment variables and the image allowlist.
//!
//! ```toml
//! # /etc/necropsy/images.toml
//! max_instances = 40
//!
//! [images.webshell-lab]
//! image = "necropsy-range/webshell-lab:1"
//! memory_mb = 128      # hard limit, swap included; the tmpfs counts toward it
//! cpus = 0.5
//! pids = 64
//! tmpfs_mb = 16        # the only writable place in the container
//! uid = 10001          # the user the image runs as
//! ```

use std::collections::BTreeMap;
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
struct Images {
    #[serde(default = "d_max")]
    max_instances: usize,
    images: BTreeMap<String, Image>,
}

fn d_max() -> usize {
    20
}

#[derive(Debug, Clone)]
pub struct Config {
    pub socket: PathBuf,
    /// The group allowed to use the socket (the web server's gid).
    pub socket_gid: u32,
    pub range_dir: PathBuf,
    pub docker: String,
    pub max_instances: usize,
    pub images: BTreeMap<String, Image>,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let var = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_owned());
        let images_path = var("SPAWNER_IMAGES", "/etc/necropsy/images.toml");
        let text = std::fs::read_to_string(&images_path)
            .map_err(|e| format!("cannot read {images_path}: {e}"))?;
        let (max, images) = parse_images(&text).map_err(|e| format!("{images_path}: {e}"))?;
        Ok(Self {
            socket: var("SPAWNER_SOCKET", "/run/necropsy/spawner.sock").into(),
            socket_gid: var("SPAWNER_SOCKET_GID", "65532")
                .parse()
                .map_err(|_| "SPAWNER_SOCKET_GID must be a number".to_owned())?,
            range_dir: var("SPAWNER_RANGE_DIR", "/run/necropsy/range").into(),
            docker: var("SPAWNER_DOCKER", "docker"),
            max_instances: var("SPAWNER_MAX_INSTANCES", &max.to_string())
                .parse()
                .map_err(|_| "SPAWNER_MAX_INSTANCES must be a number".to_owned())?,
            images,
        })
    }
}

pub fn parse_images(text: &str) -> Result<(usize, BTreeMap<String, Image>), String> {
    let f: Images = toml::from_str(text).map_err(|e| e.to_string())?;
    for (name, i) in &f.images {
        let ok_name = !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        // The image reference is passed to `docker run` as an argument, but
        // an option-looking one would still be read as a flag.
        let ok_ref = !i.image.is_empty()
            && !i.image.starts_with('-')
            && i.image.bytes().all(|b| b.is_ascii_graphic());
        if !ok_name || !ok_ref {
            return Err(format!("bad image entry {name:?}"));
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
    Ok((f.max_instances, f.images))
}
