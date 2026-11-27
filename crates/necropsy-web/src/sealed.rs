// SPDX-License-Identifier: AGPL-3.0-or-later
//! Sealed packs on the site: opened at startup with the master key, checked,
//! and served like any other daily from their date. Each pack's own key is
//! published once its date is over in New York. See `docs/sealing.md`.
//!
//! A sealed pack that cannot be opened or fails its checks is skipped, the
//! admin page says why, and a fallback pack from the manifest takes its date.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use axum::extract::{Path as UrlPath, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use necropsy_pack::seal::{self, Manifest, MasterKeys, WrapKey};
use necropsy_pack::{gen, Pack};
use serde_json::json;

use crate::state::AppState;

/// The key the site checks generators with: not the server secret, so no check
/// output is a real player's.
const CHECK_KEY: &[u8] = b"necropsy site check";

/// A loaded sealed pack's key and the date it is released for.
struct Loaded {
    k: [u8; 32],
    day: i64,
    date: String,
    key_version: u32,
}

/// What happened to one manifest entry, for the admin page.
#[derive(Clone, Debug)]
pub struct Status {
    pub id: String,
    pub date: Option<String>,
    /// `ok`, `fallback for <id>`, `spare fallback`, or why it was skipped.
    pub outcome: String,
    pub ok: bool,
}

#[derive(Default)]
pub struct Sealed {
    /// The wrap key packs are sealed to now; `None` without a master key.
    pub wrap_key: Option<WrapKey>,
    loaded: HashMap<String, Loaded>,
    pub status: Vec<Status>,
}

impl Sealed {
    /// Packs sealed and released, with their keys: those whose date is over in New York.
    fn released(&self, now: i64) -> Vec<(&String, &Loaded)> {
        let mut v: Vec<_> = self
            .loaded
            .iter()
            .filter(|(_, l)| now >= seal::released_at(l.day))
            .collect();
        v.sort_by_key(|(_, l)| l.day);
        v
    }

    /// Sealed packs a site has open, released or not.
    pub fn count(&self) -> usize {
        self.loaded.len()
    }
}

/// A fallback pack that has not been used: its entry, files and key.
type Spare = (seal::Entry, Vec<(String, Vec<u8>)>, [u8; 32]);

/// Opens, checks and loads the sealed packs in `dir`. Returns the packs to
/// serve and what happened to each. Without a master key nothing is opened.
pub fn load(
    dir: &Path,
    master: Option<&MasterKeys>,
    plain: &[Pack],
    runner: &mut gen::Runner<'_>,
) -> (Vec<Pack>, Sealed) {
    let mut out = Sealed::default();
    let manifest = match Manifest::load(dir) {
        Ok(m) => m,
        Err(e) => {
            out.status.push(Status {
                id: "manifest.toml".into(),
                date: None,
                outcome: e,
                ok: false,
            });
            return (Vec::new(), out);
        }
    };
    let Some(master) = master else {
        for e in &manifest.pack {
            out.status.push(Status {
                id: e.id.clone(),
                date: e.date.clone(),
                outcome: "not opened: this site has no master key".into(),
                ok: false,
            });
        }
        return (Vec::new(), out);
    };
    out.wrap_key = Some(master.current_wrap_key());

    // Open and check every entry. A fallback is checked with a stand-in date,
    // and loaded again with the date it takes.
    let mut packs: Vec<Pack> = Vec::new();
    let mut failed: Vec<(seal::Entry, String)> = Vec::new();
    let mut spares: Vec<Spare> = Vec::new();
    let mut dates: HashMap<String, String> = plain
        .iter()
        .filter_map(|p| p.meta.date.clone().map(|d| (d, p.meta.id.clone())))
        .collect();
    for e in &manifest.pack {
        let opened = open_entry(dir, e, master, plain, runner);
        match opened {
            Ok((files, k, pack)) if e.fallback => {
                drop(pack);
                spares.push((e.clone(), files, k));
            }
            Ok((_, k, pack)) => {
                let date = e.date.clone().unwrap_or_default();
                if let Some(other) = dates.insert(date.clone(), e.id.clone()) {
                    failed.push((e.clone(), format!("{date} is already {other}'s date")));
                    continue;
                }
                let day = necropsy_pack::parse_date(&date).unwrap_or(0);
                out.loaded.insert(
                    e.id.clone(),
                    Loaded {
                        k,
                        day,
                        date: date.clone(),
                        key_version: e.key_version,
                    },
                );
                out.status.push(Status {
                    id: e.id.clone(),
                    date: Some(date),
                    outcome: "ok".into(),
                    ok: true,
                });
                packs.push(pack);
            }
            Err(why) => failed.push((e.clone(), why)),
        }
    }

    // A dated pack that failed: tell the admin, and give its date and number to
    // the next fallback, in date order.
    failed.sort_by(|a, b| a.0.date.cmp(&b.0.date));
    spares.reverse();
    for (e, why) in failed {
        tracing::error!("sealed pack {} was skipped: {why}", e.id);
        out.status.push(Status {
            id: e.id.clone(),
            date: e.date.clone(),
            outcome: format!("skipped: {why}"),
            ok: false,
        });
        let Some(date) = e.date.clone() else { continue };
        let mut replaced = false;
        while let Some((fe, files, k)) = spares.pop() {
            let label = dir.join(format!("{}.sealed", fe.id));
            match necropsy_pack::load_pack_files(&label, files, Some(&date)) {
                Ok(mut pack) => {
                    pack.meta.number = e.number;
                    tracing::warn!("fallback {} takes {date}, in place of {}", fe.id, e.id);
                    out.loaded.insert(
                        fe.id.clone(),
                        Loaded {
                            k,
                            day: necropsy_pack::parse_date(&date).unwrap_or(0),
                            date: date.clone(),
                            key_version: fe.key_version,
                        },
                    );
                    out.status.push(Status {
                        id: fe.id.clone(),
                        date: Some(date.clone()),
                        outcome: format!("fallback for {}", e.id),
                        ok: true,
                    });
                    packs.push(pack);
                    replaced = true;
                    break;
                }
                Err(err) => out.status.push(Status {
                    id: fe.id.clone(),
                    date: None,
                    outcome: format!("skipped fallback: {err}"),
                    ok: false,
                }),
            }
        }
        if !replaced {
            tracing::error!("no fallback is left for {date}: there is no daily that day");
            out.status.push(Status {
                id: e.id.clone(),
                date: Some(date),
                outcome: "no fallback left: there is no daily that day".into(),
                ok: false,
            });
        }
    }
    for (fe, _, _) in spares {
        out.status.push(Status {
            id: fe.id,
            date: None,
            outcome: "spare fallback".into(),
            ok: true,
        });
    }
    out.status
        .sort_by(|a, b| (&a.date, &a.id).cmp(&(&b.date, &b.id)));
    (packs, out)
}

type Opened = (Vec<(String, Vec<u8>)>, [u8; 32], Pack);

/// Opens one entry and checks it as CI cannot: it decrypts, loads, agrees with
/// its manifest entry, and its generator passes the same checks as `check-gen`.
fn open_entry(
    dir: &Path,
    e: &seal::Entry,
    master: &MasterKeys,
    plain: &[Pack],
    runner: &mut gen::Runner<'_>,
) -> Result<Opened, String> {
    let path = dir.join(format!("{}.sealed", e.id));
    let bytes = std::fs::read(&path).map_err(|err| format!("{}: {err}", path.display()))?;
    if seal::sha256_hex(&bytes) != e.sha256 {
        return Err("the file does not match its manifest sha256".into());
    }
    let s = seal::parse(&bytes)?;
    if s.header.pack_id != e.id {
        return Err(format!("the file is sealed as {}", s.header.pack_id));
    }
    let k = seal::unwrap_key(&s, master)?;
    let files = seal::open(&s, &k)?;
    let stand_in = e.fallback.then_some("2000-01-01");
    let pack = necropsy_pack::load_pack_files(&path, files.clone(), stand_in)
        .map_err(|err| err.to_string())?;
    let m = &pack.meta;
    if m.id != e.id
        || m.difficulty != e.difficulty
        || (!e.fallback && (m.number != e.number || m.date != e.date))
    {
        return Err("its pack.toml does not match its manifest entry".into());
    }
    if m.track != necropsy_pack::Track::Daily {
        return Err("a sealed pack must be a daily".into());
    }
    if plain.iter().any(|p| p.meta.id == m.id) {
        return Err("a plaintext pack has the same id".into());
    }
    if !e.fallback
        && plain
            .iter()
            .any(|p| p.meta.track == m.track && p.meta.number == m.number)
    {
        return Err(format!("daily number {} is already taken", m.number));
    }
    if pack.is_generated() {
        gen::check_with(&pack, &gen::plaintext(&files), CHECK_KEY, runner)
            .map_err(|err| format!("its generator fails its checks: {err}"))?;
    }
    Ok((files, k, pack))
}

/// Where the master key comes from (docs/sealing.md): `NECROPSY_MASTER_KEY_FILE`
/// if set; else `NECROPSY_MASTER_KEY`; else `default_file`. A key file that does
/// not exist is made, with a new random key, mode 0600. Returns the keys and
/// whether they were just made.
pub fn load_master(
    file_var: Option<String>,
    env_var: Option<String>,
    default_file: &Path,
) -> Result<(MasterKeys, bool), String> {
    let nonempty = |v: Option<String>| v.filter(|v| !v.trim().is_empty());
    let path = match (nonempty(file_var), nonempty(env_var)) {
        (Some(f), _) => std::path::PathBuf::from(f.trim()),
        (None, Some(v)) => return MasterKeys::parse(&v).map(|m| (m, false)),
        (None, None) => default_file.to_path_buf(),
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => MasterKeys::parse(&text)
            .map(|m| (m, false))
            .map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let text = MasterKeys::generate_text();
            crate::generated::create_private(&path, &format!("{text}\n"))?;
            MasterKeys::parse(&text).map(|m| (m, true))
        }
        Err(e) => Err(format!(
            "cannot read the master key {}: {e}",
            path.display()
        )),
    }
}

/// `2026-12-05 05:00 UTC`.
pub fn utc_time(secs: i64) -> String {
    let (y, m, d) = necropsy_pack::civil_from_days(secs.div_euclid(86_400));
    let s = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} UTC",
        s / 3600,
        s % 3600 / 60
    )
}

// ---------------------------------------------------------------- endpoints

/// `GET /api/packs/wrap-key`: the public key packs are sealed to now.
pub async fn wrap_key(State(state): State<Arc<AppState>>) -> Response {
    let Some(k) = &state.sealed.wrap_key else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "this site has no pack key"})),
        )
            .into_response();
    };
    (
        [(header::CACHE_CONTROL, "public, max-age=300")],
        Json(json!({
            "version": k.version,
            "public_key": k.public_hex(),
            "fingerprint": k.fingerprint_hex(),
            "scheme": "necropsy-seal-v1: each pack has a random 256-bit key K; K is sealed to this X25519 key (libsodium crypto_box_seal) and the pack is encrypted with K (AES-256-GCM, the header as associated data)",
            "docs": "docs/sealing.md",
        })),
    )
        .into_response()
}

/// `GET /api/packs/keys`: every released pack's key, and when the others are due.
pub async fn keys(State(state): State<Arc<AppState>>) -> Response {
    let now = state.now_secs();
    let released: Vec<_> = state
        .sealed
        .released(now)
        .into_iter()
        .map(|(id, l)| {
            json!({
                "id": id,
                "date": l.date,
                "key": seal::key_hex(&l.k),
                "key_version": l.key_version,
                "released_at": utc_time(seal::released_at(l.day)),
            })
        })
        .collect();
    let mut upcoming: Vec<_> = state
        .sealed
        .loaded
        .iter()
        .filter(|(_, l)| now < seal::released_at(l.day))
        .map(|(id, l)| (l.day, json!({"id": id, "date": l.date, "released_at": utc_time(seal::released_at(l.day))})))
        .collect();
    upcoming.sort_by_key(|(d, _)| *d);
    let upcoming: Vec<_> = upcoming.into_iter().map(|(_, v)| v).collect();
    Json(json!({"released": released, "upcoming": upcoming})).into_response()
}

/// `GET /api/packs/{id}/key`: a released pack's key, or when it will be.
pub async fn key(State(state): State<Arc<AppState>>, UrlPath(id): UrlPath<String>) -> Response {
    let Some(l) = state.sealed.loaded.get(&id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "no sealed pack with that id"})),
        )
            .into_response();
    };
    let at = seal::released_at(l.day);
    if state.now_secs() < at {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": "not released yet",
                "released_at": utc_time(at),
            })),
        )
            .into_response();
    }
    Json(json!({
        "id": id,
        "date": l.date,
        "key": seal::key_hex(&l.k),
        "key_version": l.key_version,
        "released_at": utc_time(at),
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_are_written_in_utc() {
        let day = necropsy_pack::parse_date("2026-12-05").unwrap();
        assert_eq!(utc_time(day * 86_400 + 5 * 3600), "2026-12-05 05:00 UTC");
    }
}
