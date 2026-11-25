// SPDX-License-Identifier: AGPL-3.0-or-later
//! Images pushed by a site. A site builds its labs and streams each one here as
//! a `docker save` archive. Before anything is loaded, the archive's own list of
//! tags is read and must name only this tenant's labs: loading an archive
//! applies the tags it carries, so one that claimed another tenant's tag would
//! replace that tenant's lab.

use std::io::Read;
use std::path::Path;

use crate::config::valid_image_ref;

/// Largest archive accepted.
pub const MAX_BYTES: u64 = 1024 * 1024 * 1024;
/// Largest manifest read from it.
const MAX_MANIFEST: u64 = 1024 * 1024;

/// The tags the archive at `path` would create, if every one is under
/// `necropsy-range/<tenant>/`.
pub fn check_archive(path: &Path, tenant: &str) -> Result<Vec<String>, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut archive = tar::Archive::new(file);
    let prefix = format!("necropsy-range/{tenant}/");
    let entries = archive
        .entries_with_seek()
        .map_err(|_| "not a docker archive".to_owned())?;
    for entry in entries {
        let entry = entry.map_err(|_| "not a docker archive".to_owned())?;
        let is_manifest = entry
            .path()
            .map(|p| p == Path::new("manifest.json"))
            .unwrap_or(false);
        if !is_manifest {
            continue;
        }
        let mut text = String::new();
        entry
            .take(MAX_MANIFEST)
            .read_to_string(&mut text)
            .map_err(|_| "unreadable manifest".to_owned())?;
        return tags_of(&text, &prefix);
    }
    Err("not a docker archive: no manifest.json".into())
}

fn tags_of(manifest: &str, prefix: &str) -> Result<Vec<String>, String> {
    let v: serde_json::Value =
        serde_json::from_str(manifest).map_err(|_| "unreadable manifest".to_owned())?;
    let mut tags = Vec::new();
    for image in v.as_array().ok_or("unreadable manifest")? {
        let list = image
            .get("RepoTags")
            .and_then(|t| t.as_array())
            .ok_or("the archive has an image with no tag")?;
        if list.is_empty() {
            return Err("the archive has an image with no tag".into());
        }
        for t in list {
            let t = t.as_str().ok_or("bad tag")?;
            if !valid_image_ref(t, prefix) {
                return Err(format!(
                    "the archive carries the tag {t:?}, which is not under {prefix}"
                ));
            }
            tags.push(t.to_owned());
        }
    }
    if tags.is_empty() {
        return Err("the archive has no images".into());
    }
    Ok(tags)
}
