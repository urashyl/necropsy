// SPDX-License-Identifier: AGPL-3.0-or-later
//! Sealed packs: unreleased packs, encrypted in the public repository. See
//! `docs/sealing.md`.
//!
//! - One **master key** (production's secret, never in the repository or CI)
//!   derives a **wrap key pair** per version: the secret half is
//!   `HKDF-SHA256(master, "necropsy:pack-wrap:v<N>")` used as an X25519 key; the
//!   public half is committed (`packs-sealed/wrap-keys/v<N>.pub`) and served at
//!   `/api/packs/wrap-key`.
//! - Each pack has its **own random 256-bit key** K. The pack is encrypted with
//!   K (AES-256-GCM) and K is sealed to the wrap public key (an X25519 sealed
//!   box, as libsodium's `crypto_box_seal`). Both are in the `.sealed` file.
//! - The **header** (pack id, wrap key version and fingerprint, nonce) is the
//!   AEAD's associated data, so a K opens exactly the pack it was made for: a
//!   header moved onto another pack's ciphertext fails, and every other pack
//!   has a K of its own.
//! - After a daily's date is over in New York, the site publishes its K
//!   ([`released_at`]); with it anyone can open that pack, and no other.

use std::collections::BTreeMap;
use std::path::Path;

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use crypto_box::{PublicKey, SecretKey};
use rand_core::{OsRng, RngCore};
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// The start of every `.sealed` file.
pub const MAGIC: &[u8; 8] = b"NPSEAL1\0";
/// Size of a sealed K: an ephemeral X25519 public key, K, and a tag.
const WRAPPED_LEN: usize = 32 + 32 + 16;
/// Files and bytes an archive may hold: the same as a pack directory.
const MAX_FILES: usize = 256;
const MAX_FILE: usize = 1 << 20;
const MAX_TOTAL: usize = 8 << 20;

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

fn key32(b: &[u8]) -> Option<[u8; 32]> {
    b.try_into().ok()
}

/// Random bytes from the operating system.
pub fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    OsRng.fill_bytes(&mut b);
    b
}

// ---------------------------------------------------------------- keys

/// The master keys a site holds, by version. Written `1:<64 hex>`, several
/// comma- or newline-separated during a rotation (`1:...,2:...`); a bare
/// `<64 hex>` is version 1. The highest version seals; every version opens.
#[derive(Clone)]
pub struct MasterKeys(BTreeMap<u32, [u8; 32]>);

impl std::fmt::Debug for MasterKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "MasterKeys(versions {:?})",
            self.0.keys().collect::<Vec<_>>()
        )
    }
}

impl MasterKeys {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut keys = BTreeMap::new();
        for part in text
            .split([',', '\n'])
            .map(str::trim)
            .filter(|p| !p.is_empty() && !p.starts_with('#'))
        {
            let (v, h) = match part.split_once(':') {
                Some((v, h)) => (
                    v.trim()
                        .parse::<u32>()
                        .map_err(|_| format!("{v:?} is not a key version"))?,
                    h,
                ),
                None => (1, part),
            };
            let k = unhex(h)
                .and_then(|b| key32(&b))
                .ok_or_else(|| format!("master key v{v} is not 64 hex digits"))?;
            if keys.insert(v, k).is_some() {
                return Err(format!("master key v{v} is given twice"));
            }
        }
        if keys.is_empty() {
            return Err("no master key".into());
        }
        Ok(Self(keys))
    }

    /// From `NECROPSY_MASTER_KEY_FILE` (a file) or `NECROPSY_MASTER_KEY`, if set.
    pub fn from_env() -> Result<Option<Self>, String> {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        if let Some(path) = get("NECROPSY_MASTER_KEY_FILE") {
            let text = std::fs::read_to_string(path.trim())
                .map_err(|e| format!("NECROPSY_MASTER_KEY_FILE {}: {e}", path.trim()))?;
            return Self::parse(&text).map(Some);
        }
        match get("NECROPSY_MASTER_KEY") {
            Some(v) => Self::parse(&v).map(Some),
            None => Ok(None),
        }
    }

    /// A new random master key, as `1:<hex>`.
    pub fn generate_text() -> String {
        format!("1:{}", hex(&random::<32>()))
    }

    pub fn current_version(&self) -> u32 {
        *self.0.keys().next_back().expect("not empty")
    }

    pub fn versions(&self) -> Vec<u32> {
        self.0.keys().copied().collect()
    }

    fn wrap_secret(&self, version: u32) -> Option<SecretKey> {
        let master = self.0.get(&version)?;
        let hk = hkdf::Hkdf::<Sha256>::new(None, master);
        let mut okm = [0u8; 32];
        hk.expand(
            format!("necropsy:pack-wrap:v{version}").as_bytes(),
            &mut okm,
        )
        .expect("32 bytes is a valid HKDF length");
        Some(SecretKey::from(okm))
    }

    /// The public wrap key of a version.
    pub fn wrap_key(&self, version: u32) -> Option<WrapKey> {
        self.wrap_secret(version).map(|s| WrapKey {
            version,
            public: *s.public_key().as_bytes(),
        })
    }

    /// The wrap key packs are sealed to now.
    pub fn current_wrap_key(&self) -> WrapKey {
        self.wrap_key(self.current_version())
            .expect("current version exists")
    }
}

/// A public wrap key and its version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WrapKey {
    pub version: u32,
    pub public: [u8; 32],
}

impl WrapKey {
    /// The first 8 bytes of SHA-256 of the key, kept in each sealed header so
    /// CI can check which key a pack was sealed to.
    pub fn fingerprint(&self) -> [u8; 8] {
        Sha256::digest(self.public)[..8]
            .try_into()
            .expect("8 bytes")
    }

    pub fn public_hex(&self) -> String {
        hex(&self.public)
    }

    pub fn fingerprint_hex(&self) -> String {
        hex(&self.fingerprint())
    }

    /// The committed file's contents: `v1 <64 hex>`.
    pub fn to_file(&self) -> String {
        format!("v{} {}\n", self.version, self.public_hex())
    }

    pub fn parse_file(text: &str) -> Result<Self, String> {
        let line = text
            .lines()
            .find(|l| !l.trim().is_empty() && !l.starts_with('#'))
            .unwrap_or("");
        let (v, h) = line
            .trim()
            .split_once(' ')
            .ok_or("a wrap key file is `v<N> <64 hex>`")?;
        let version = v
            .strip_prefix('v')
            .and_then(|v| v.parse().ok())
            .ok_or("a wrap key file is `v<N> <64 hex>`")?;
        let public = unhex(h)
            .and_then(|b| key32(&b))
            .ok_or("the key is not 64 hex digits")?;
        Ok(Self { version, public })
    }

    /// From the JSON `/api/packs/wrap-key` serves.
    pub fn from_json(v: &serde_json::Value) -> Result<Self, String> {
        let version = v["version"].as_u64().ok_or("no version")? as u32;
        let public = v["public_key"]
            .as_str()
            .and_then(unhex)
            .and_then(|b| key32(&b))
            .ok_or("no public_key")?;
        let key = Self { version, public };
        if v["fingerprint"].as_str() != Some(key.fingerprint_hex().as_str()) {
            return Err("the fingerprint does not match the key".into());
        }
        Ok(key)
    }
}

/// The committed wrap keys, `packs-sealed/wrap-keys/v<N>.pub`, by version.
pub fn committed_wrap_keys(sealed_dir: &Path) -> Result<BTreeMap<u32, WrapKey>, String> {
    let mut out = BTreeMap::new();
    let dir = sealed_dir.join("wrap-keys");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(out);
    };
    for e in entries.flatten() {
        let p = e.path();
        let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        let k = WrapKey::parse_file(&text).map_err(|e| format!("{}: {e}", p.display()))?;
        let name = format!("v{}.pub", k.version);
        if p.file_name().and_then(|n| n.to_str()) != Some(name.as_str()) {
            return Err(format!(
                "{} holds v{}: name it {name}",
                p.display(),
                k.version
            ));
        }
        out.insert(k.version, k);
    }
    Ok(out)
}

// ---------------------------------------------------------------- the file

/// What a sealed file says about itself, in the clear.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub pack_id: String,
    pub key_version: u32,
    pub fingerprint: [u8; 8],
    pub nonce: [u8; 12],
}

impl Header {
    fn encode(&self) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        out.push(self.pack_id.len() as u8);
        out.extend_from_slice(self.pack_id.as_bytes());
        out.extend_from_slice(&self.key_version.to_le_bytes());
        out.extend_from_slice(&self.fingerprint);
        out.extend_from_slice(&self.nonce);
        out
    }
}

/// A parsed `.sealed` file, not yet opened.
pub struct Sealed {
    pub header: Header,
    header_bytes: Vec<u8>,
    wrapped: Vec<u8>,
    ciphertext: Vec<u8>,
}

/// Reads a `.sealed` file's structure. Opens nothing: CI uses it.
pub fn parse(b: &[u8]) -> Result<Sealed, String> {
    let rest = b
        .strip_prefix(MAGIC.as_slice())
        .ok_or("not a sealed pack: no NPSEAL1 header")?;
    let id_len = *rest.first().ok_or("the header ends early")? as usize;
    let need = 1 + id_len + 4 + 8 + 12 + WRAPPED_LEN + 16;
    if rest.len() < need {
        return Err("the file ends early".into());
    }
    let pack_id = std::str::from_utf8(&rest[1..1 + id_len])
        .map_err(|_| "the pack id is not UTF-8")?
        .to_owned();
    if !crate::valid_id(&pack_id) {
        return Err(format!("{pack_id:?} is not a pack id"));
    }
    let mut at = 1 + id_len;
    let key_version = u32::from_le_bytes(rest[at..at + 4].try_into().expect("4"));
    at += 4;
    let fingerprint: [u8; 8] = rest[at..at + 8].try_into().expect("8");
    at += 8;
    let nonce: [u8; 12] = rest[at..at + 12].try_into().expect("12");
    at += 12;
    let header_bytes = b[..MAGIC.len() + at].to_vec();
    let wrapped = rest[at..at + WRAPPED_LEN].to_vec();
    at += WRAPPED_LEN;
    Ok(Sealed {
        header: Header {
            pack_id,
            key_version,
            fingerprint,
            nonce,
        },
        header_bytes,
        wrapped,
        ciphertext: rest[at..].to_vec(),
    })
}

/// Seals a pack's files to a wrap key. Returns the `.sealed` bytes and the pack's
/// own key K (which the caller does not need to keep: the site unwraps it).
pub fn seal(
    pack_id: &str,
    files: &[(String, Vec<u8>)],
    wrap: &WrapKey,
) -> Result<(Vec<u8>, [u8; 32]), String> {
    let k = random::<32>();
    Ok((seal_with(pack_id, files, wrap, &k)?, k))
}

fn seal_with(
    pack_id: &str,
    files: &[(String, Vec<u8>)],
    wrap: &WrapKey,
    k: &[u8; 32],
) -> Result<Vec<u8>, String> {
    if !crate::valid_id(pack_id) {
        return Err(format!("{pack_id:?} is not a pack id"));
    }
    let header = Header {
        pack_id: pack_id.to_owned(),
        key_version: wrap.version,
        fingerprint: wrap.fingerprint(),
        nonce: random::<12>(),
    };
    let header_bytes = header.encode();
    let wrapped = PublicKey::from(wrap.public)
        .seal(&mut OsRng, k)
        .map_err(|_| "sealing the key failed")?;
    let plain = archive(files)?;
    let ct = Aes256Gcm::new_from_slice(k)
        .expect("32-byte key")
        .encrypt(
            Nonce::from_slice(&header.nonce),
            Payload {
                msg: &plain,
                aad: &header_bytes,
            },
        )
        .map_err(|_| "encrypting the pack failed")?;
    let mut out = header_bytes;
    out.extend_from_slice(&wrapped);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Unwraps a pack's K with the master key of the version it was sealed to.
pub fn unwrap_key(s: &Sealed, master: &MasterKeys) -> Result<[u8; 32], String> {
    let secret = master.wrap_secret(s.header.key_version).ok_or_else(|| {
        format!(
            "no master key v{} to open {}",
            s.header.key_version, s.header.pack_id
        )
    })?;
    let want = master
        .wrap_key(s.header.key_version)
        .expect("exists")
        .fingerprint();
    if want != s.header.fingerprint {
        return Err(format!(
            "{} was sealed to another v{} key than this site's",
            s.header.pack_id, s.header.key_version
        ));
    }
    let k = secret.unseal(&s.wrapped).map_err(|_| {
        format!(
            "{}'s key does not unwrap with this site's master key",
            s.header.pack_id
        )
    })?;
    key32(&k).ok_or_else(|| "the unwrapped key is not 32 bytes".to_owned())
}

/// Opens a pack with its K: the files, or an error if K is not this pack's.
pub fn open(s: &Sealed, k: &[u8; 32]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let plain = Aes256Gcm::new_from_slice(k)
        .expect("32-byte key")
        .decrypt(
            Nonce::from_slice(&s.header.nonce),
            Payload {
                msg: &s.ciphertext,
                aad: &s.header_bytes,
            },
        )
        .map_err(|_| format!("this key does not open {}", s.header.pack_id))?;
    unarchive(&plain)
}

/// Seals a pack again, to a newer wrap key, with the same K: for a master key
/// rotation. A released K keeps opening it.
pub fn rewrap(b: &[u8], master: &MasterKeys, to: &WrapKey) -> Result<Vec<u8>, String> {
    let s = parse(b)?;
    let k = unwrap_key(&s, master)?;
    let files = open(&s, &k)?;
    seal_with(&s.header.pack_id, &files, to, &k)
}

/// `NPPACK1`, a count, then each path and its bytes, every length a u32 (LE).
fn archive(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>, String> {
    let mut out = b"NPPACK1".to_vec();
    out.extend_from_slice(&(files.len() as u32).to_le_bytes());
    for (p, d) in files {
        out.extend_from_slice(&(p.len() as u32).to_le_bytes());
        out.extend_from_slice(p.as_bytes());
        out.extend_from_slice(&(d.len() as u32).to_le_bytes());
        out.extend_from_slice(d);
    }
    Ok(out)
}

fn unarchive(b: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut at = b
        .strip_prefix(b"NPPACK1")
        .ok_or("the opened pack is not an archive")?;
    let take = |at: &mut &[u8]| -> Result<Vec<u8>, String> {
        if at.len() < 4 {
            return Err("the archive ends early".into());
        }
        let n = u32::from_le_bytes(at[..4].try_into().expect("4")) as usize;
        if at.len() < 4 + n {
            return Err("the archive ends early".into());
        }
        let v = at[4..4 + n].to_vec();
        *at = &at[4 + n..];
        Ok(v)
    };
    if at.len() < 4 {
        return Err("the archive ends early".into());
    }
    let count = u32::from_le_bytes(at[..4].try_into().expect("4")) as usize;
    at = &at[4..];
    if count > MAX_FILES {
        return Err("the archive has too many files".into());
    }
    let mut files = Vec::with_capacity(count);
    let mut total = 0;
    for _ in 0..count {
        let path = String::from_utf8(take(&mut at)?).map_err(|_| "a path is not UTF-8")?;
        if !crate::gen::valid_path(&path) {
            return Err(format!("{path:?} is not a plain relative path"));
        }
        let data = take(&mut at)?;
        if data.len() > MAX_FILE {
            return Err(format!("{path} is larger than {MAX_FILE} bytes"));
        }
        total += data.len();
        if total > MAX_TOTAL {
            return Err("the archive is too large".into());
        }
        files.push((path, data));
    }
    if !at.is_empty() {
        return Err("the archive has bytes after the last file".into());
    }
    Ok(files)
}

/// A key as hex, for publishing.
pub fn key_hex(k: &[u8; 32]) -> String {
    hex(k)
}

pub fn key_from_hex(s: &str) -> Option<[u8; 32]> {
    unhex(s).and_then(|b| key32(&b))
}

/// SHA-256 of a file, as the manifest records it.
pub fn sha256_hex(b: &[u8]) -> String {
    hex(&Sha256::digest(b))
}

// ---------------------------------------------------------------- release time

/// Whether US Eastern time is on daylight saving at local midnight starting
/// `day` (days since 1970). DST runs from 02:00 on the second Sunday of March
/// to 02:00 on the first Sunday of November, so midnight of the March Sunday is
/// still standard time and midnight of the November Sunday is still daylight time.
fn ny_dst_at_midnight(day: i64) -> bool {
    let (y, _, _) = crate::civil_from_days(day);
    let second_sunday_march = {
        let first = crate::days_from_civil(y, 3, 1);
        let dow = (first + 4).rem_euclid(7); // 0 = Sunday
        first + (7 - dow) % 7 + 7
    };
    let first_sunday_november = {
        let first = crate::days_from_civil(y, 11, 1);
        let dow = (first + 4).rem_euclid(7);
        first + (7 - dow) % 7
    };
    day > second_sunday_march && day <= first_sunday_november
}

/// When a daily dated `day` expires and its key is published: midnight at the
/// end of its date in New York (America/New_York), in seconds since the epoch.
pub fn released_at(day: i64) -> i64 {
    let next = day + 1;
    let offset_hours = if ny_dst_at_midnight(next) { 4 } else { 5 };
    next * 86_400 + offset_hours * 3600
}

// ---------------------------------------------------------------- the manifest

/// One sealed pack in `packs-sealed/manifest.toml`: what CI can check without
/// opening it.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub id: String,
    pub number: u32,
    pub difficulty: u8,
    /// The release date. Absent for a fallback.
    #[serde(default)]
    pub date: Option<String>,
    /// Released in place of a dated pack that fails its checks.
    #[serde(default)]
    pub fallback: bool,
    pub key_version: u32,
    pub sha256: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    #[serde(default)]
    pub pack: Vec<Entry>,
}

impl Manifest {
    pub fn load(sealed_dir: &Path) -> Result<Self, String> {
        let path = sealed_dir.join("manifest.toml");
        match std::fs::read_to_string(&path) {
            Ok(t) => toml::from_str(&t).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// The manifest as TOML, entries in date order with fallbacks last.
    pub fn to_toml(&self) -> String {
        let mut entries = self.pack.clone();
        entries.sort_by(|a, b| (a.fallback, &a.date, &a.id).cmp(&(b.fallback, &b.date, &b.id)));
        let mut out = String::from(
            "# Sealed packs: one entry per packs-sealed/<id>.sealed. Written by `pack-tool seal`.\n# See docs/sealing.md.\n",
        );
        for e in entries {
            out.push_str("\n[[pack]]\n");
            out.push_str(&format!(
                "id = \"{}\"\nnumber = {}\ndifficulty = {}\n",
                e.id, e.number, e.difficulty
            ));
            if let Some(d) = &e.date {
                out.push_str(&format!("date = \"{d}\"\n"));
            }
            if e.fallback {
                out.push_str("fallback = true\n");
            }
            out.push_str(&format!(
                "key_version = {}\nsha256 = \"{}\"\n",
                e.key_version, e.sha256
            ));
        }
        out
    }

    pub fn upsert(&mut self, e: Entry) {
        self.pack.retain(|x| x.id != e.id);
        self.pack.push(e);
    }
}

/// Every check CI makes on `packs-sealed/`, with no secret: each file is a
/// sealed pack in the current format sealed to the current committed wrap key,
/// each matches its manifest entry and the other way round, and no id, number
/// or date collides with another pack's. `plain` are the plaintext packs.
pub fn check_dir(sealed_dir: &Path, plain: &[crate::Pack]) -> Result<Vec<String>, String> {
    let manifest = Manifest::load(sealed_dir)?;
    let keys = committed_wrap_keys(sealed_dir)?;
    let mut report = Vec::new();
    let current = keys.values().next_back();
    if !manifest.pack.is_empty() && current.is_none() {
        return Err("sealed packs need a committed wrap key in packs-sealed/wrap-keys/".into());
    }
    // Only sealed packs, the manifest and the keys belong here.
    if let Ok(entries) = std::fs::read_dir(sealed_dir) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let ok = name == "manifest.toml"
                || name == "README.md"
                || (name == "wrap-keys" && e.path().is_dir())
                || (name.ends_with(".sealed") && e.path().is_file());
            if !ok {
                return Err(format!(
                    "packs-sealed/{name} does not belong here: only .sealed files, manifest.toml and wrap-keys/ (a plaintext pack goes in packs-src/, which is never committed)"
                ));
            }
            if let Some(id) = name.strip_suffix(".sealed") {
                if !manifest.pack.iter().any(|m| m.id == id) {
                    return Err(format!("packs-sealed/{name} has no entry in manifest.toml"));
                }
            }
        }
    }
    let mut ids = std::collections::HashSet::new();
    let mut dates: BTreeMap<String, String> = plain
        .iter()
        .filter(|p| p.meta.track == crate::Track::Daily)
        .filter_map(|p| p.meta.date.clone().map(|d| (d, p.meta.id.clone())))
        .collect();
    let mut numbers: BTreeMap<u32, String> = plain
        .iter()
        .filter(|p| p.meta.track == crate::Track::Daily)
        .map(|p| (p.meta.number, p.meta.id.clone()))
        .collect();
    for e in &manifest.pack {
        let at = format!("manifest entry {}", e.id);
        if !ids.insert(e.id.clone()) || plain.iter().any(|p| p.meta.id == e.id) {
            return Err(format!("{at}: the id {} is used twice", e.id));
        }
        if !(1..=5).contains(&e.difficulty) {
            return Err(format!("{at}: difficulty must be 1-5"));
        }
        match (&e.date, e.fallback) {
            (Some(d), false) => {
                if crate::parse_date(d).is_none() {
                    return Err(format!("{at}: {d} is not a YYYY-MM-DD date"));
                }
                if let Some(other) = dates.insert(d.clone(), e.id.clone()) {
                    return Err(format!("{at}: {d} is already {other}'s date"));
                }
                if let Some(other) = numbers.insert(e.number, e.id.clone()) {
                    return Err(format!(
                        "{at}: daily number {} is already {other}'s",
                        e.number
                    ));
                }
            }
            (None, true) => {}
            (Some(_), true) => return Err(format!("{at}: a fallback has no date")),
            (None, false) => return Err(format!("{at}: needs a date, or fallback = true")),
        }
        let path = sealed_dir.join(format!("{}.sealed", e.id));
        let bytes =
            std::fs::read(&path).map_err(|err| format!("{at}: {}: {err}", path.display()))?;
        if sha256_hex(&bytes) != e.sha256 {
            return Err(format!("{at}: sha256 does not match {}", path.display()));
        }
        let s = parse(&bytes).map_err(|err| format!("{}: {err}", path.display()))?;
        if s.header.pack_id != e.id {
            return Err(format!(
                "{}: sealed as {}, not {}",
                path.display(),
                s.header.pack_id,
                e.id
            ));
        }
        let key = keys.get(&s.header.key_version).ok_or_else(|| {
            format!(
                "{}: sealed to v{}, which is not committed",
                path.display(),
                s.header.key_version
            )
        })?;
        if key.fingerprint() != s.header.fingerprint {
            return Err(format!(
                "{}: sealed to a v{} key that is not the committed one (sealed against another site?)",
                path.display(),
                key.version
            ));
        }
        if e.key_version != s.header.key_version {
            return Err(format!(
                "{at}: key_version says v{}, the file v{}",
                e.key_version, s.header.key_version
            ));
        }
        let current = current.expect("checked above");
        if s.header.key_version != current.version {
            return Err(format!(
                "{}: sealed to v{}, but the current wrap key is v{} (run pack-tool rewrap)",
                path.display(),
                s.header.key_version,
                current.version
            ));
        }
        report.push(format!(
            "{}: sealed to v{}, {}",
            e.id,
            s.header.key_version,
            e.date.as_deref().unwrap_or("fallback")
        ));
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn master() -> MasterKeys {
        MasterKeys::parse(&format!("1:{}", "11".repeat(32))).unwrap()
    }

    fn files() -> Vec<(String, Vec<u8>)> {
        vec![
            ("pack.toml".into(), b"id = \"x\"".to_vec()),
            ("files/a.txt".into(), b"hello".to_vec()),
        ]
    }

    #[test]
    fn seal_then_open_with_the_master_key_or_the_released_key() {
        let m = master();
        let (bytes, k) = seal("demo", &files(), &m.current_wrap_key()).unwrap();
        let s = parse(&bytes).unwrap();
        assert_eq!(s.header.pack_id, "demo");
        assert_eq!(s.header.key_version, 1);
        let unwrapped = unwrap_key(&s, &m).unwrap();
        assert_eq!(unwrapped, k);
        assert_eq!(open(&s, &k).unwrap(), files());
        // Plaintext is nowhere in the file.
        assert!(!bytes.windows(5).any(|w| w == b"hello"));
    }

    #[test]
    fn a_key_opens_its_own_pack_and_no_other() {
        let m = master();
        let wk = m.current_wrap_key();
        let (a, ka) = seal("aaa", &files(), &wk).unwrap();
        let (b, kb) = seal("bbb", &files(), &wk).unwrap();
        assert_ne!(ka, kb);
        let (sa, sb) = (parse(&a).unwrap(), parse(&b).unwrap());
        assert!(open(&sb, &ka).is_err());
        assert!(open(&sa, &kb).is_err());
        // Moving a's header onto b's ciphertext does not help either: the header is authenticated.
        let mut forged = a[..sa.header_bytes.len() + WRAPPED_LEN].to_vec();
        forged.extend_from_slice(&b[sb.header_bytes.len() + WRAPPED_LEN..]);
        assert!(open(&parse(&forged).unwrap(), &ka).is_err());
        // Nor does editing the header.
        let mut edited = a.clone();
        edited[MAGIC.len() + 1] = b'z';
        assert!(open(&parse(&edited).unwrap(), &ka).is_err());
    }

    #[test]
    fn another_master_key_cannot_open_it() {
        let (bytes, _) = seal("demo", &files(), &master().current_wrap_key()).unwrap();
        let other = MasterKeys::parse(&"22".repeat(32)).unwrap();
        let e = unwrap_key(&parse(&bytes).unwrap(), &other).unwrap_err();
        assert!(e.contains("another v1 key"), "{e}");
    }

    #[test]
    fn rotation_rewraps_with_the_same_key() {
        let old = master();
        let (bytes, k) = seal("demo", &files(), &old.current_wrap_key()).unwrap();
        let both =
            MasterKeys::parse(&format!("1:{},2:{}", "11".repeat(32), "33".repeat(32))).unwrap();
        assert_eq!(both.current_version(), 2);
        let re = rewrap(&bytes, &both, &both.current_wrap_key()).unwrap();
        let s = parse(&re).unwrap();
        assert_eq!(s.header.key_version, 2);
        // Only v2 is needed to open it now, and the pack's K is the same.
        let only2 = MasterKeys::parse(&format!("2:{}", "33".repeat(32))).unwrap();
        assert_eq!(unwrap_key(&s, &only2).unwrap(), k);
        assert_eq!(open(&s, &k).unwrap(), files());
    }

    #[test]
    fn master_key_text() {
        assert!(MasterKeys::parse("").is_err());
        assert!(MasterKeys::parse("1:abc").is_err());
        assert!(MasterKeys::parse(&format!("1:{0},1:{0}", "11".repeat(32))).is_err());
        let m = MasterKeys::parse(&format!("# comment\n{}\n", "11".repeat(32))).unwrap();
        assert_eq!(m.versions(), vec![1]);
        let g = MasterKeys::generate_text();
        assert!(MasterKeys::parse(&g).is_ok());
        assert!(
            !format!("{m:?}").contains("1111"),
            "Debug never shows a key"
        );
    }

    #[test]
    fn wrap_key_files_and_json_round_trip() {
        let wk = master().current_wrap_key();
        assert_eq!(WrapKey::parse_file(&wk.to_file()).unwrap(), wk);
        let json = serde_json::json!({
            "version": 1,
            "public_key": wk.public_hex(),
            "fingerprint": hex(&wk.fingerprint()),
        });
        assert_eq!(WrapKey::from_json(&json).unwrap(), wk);
        let bad =
            serde_json::json!({"version": 1, "public_key": wk.public_hex(), "fingerprint": "00"});
        assert!(WrapKey::from_json(&bad).is_err());
    }

    #[test]
    fn malformed_files_are_refused() {
        assert!(parse(b"nope").is_err());
        let (bytes, k) = seal("demo", &files(), &master().current_wrap_key()).unwrap();
        assert!(parse(&bytes[..40]).is_err());
        let mut cut = bytes.clone();
        cut.truncate(cut.len() - 1);
        assert!(open(&parse(&cut).unwrap(), &k).is_err());
    }

    #[test]
    fn archives_refuse_unsafe_paths() {
        for bad in ["../x", "/abs", "a\\b", ""] {
            let a = archive(&[(bad.to_owned(), b"x".to_vec())]).unwrap();
            assert!(unarchive(&a).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn keys_are_released_at_midnight_in_new_york() {
        let day = |s: &str| crate::parse_date(s).unwrap();
        let utc = |s: &str, h: i64| day(s) * 86_400 + h * 3600;
        // Winter (EST, UTC-5): a 2026-12-04 daily expires at 05:00 UTC on 12-05.
        assert_eq!(released_at(day("2026-12-04")), utc("2026-12-05", 5));
        // Summer (EDT, UTC-4).
        assert_eq!(released_at(day("2026-07-01")), utc("2026-07-02", 4));
        // The switch days: 2026-03-08 and 2026-11-01 are the Sundays. Midnight
        // starting each is still the old offset.
        assert_eq!(released_at(day("2026-03-07")), utc("2026-03-08", 5));
        assert_eq!(released_at(day("2026-03-08")), utc("2026-03-09", 4));
        assert_eq!(released_at(day("2026-10-31")), utc("2026-11-01", 4));
        assert_eq!(released_at(day("2026-11-01")), utc("2026-11-02", 5));
        // 2027: DST starts 2027-03-14, ends 2027-11-07.
        assert_eq!(released_at(day("2027-03-13")), utc("2027-03-14", 5));
        assert_eq!(released_at(day("2027-03-14")), utc("2027-03-15", 4));
        assert_eq!(released_at(day("2027-11-06")), utc("2027-11-07", 4));
        assert_eq!(released_at(day("2027-11-07")), utc("2027-11-08", 5));
    }
}
