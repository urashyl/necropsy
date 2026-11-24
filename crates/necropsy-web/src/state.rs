// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared, read-only server state: packs rendered once at startup.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use necropsy_pack::Pack;
use sha2::{Digest, Sha256};

use crate::auth::{hash_password_blocking, RateLimiter};
use crate::db::Db;
use crate::mail::Mailer;
use crate::markdown::{line_count, render_markdown, Highlighter};

/// Files larger than this are offered as downloads instead of being rendered
/// inline, even when they are valid text.
const MAX_INLINE_BYTES: usize = 128 * 1024;

pub struct RenderedFile {
    pub path: String,
    pub size: usize,
    /// `Some` when the file is shown inline: highlighted HTML and line count.
    pub inline: Option<(String, usize)>,
}

pub struct RenderedPack {
    pub pack: Pack,
    pub brief_html: String,
    pub explainer_html: String,
    pub files: Vec<RenderedFile>,
}

/// Deployment settings read from the environment in `main`.
pub struct Config {
    /// Public origin used in links and share text, without a trailing slash.
    pub base_url: String,
    /// Request header carrying the client address, set by your reverse proxy
    /// (for example `X-Real-IP`). Only set it when a proxy always overwrites
    /// it, or clients can choose their own rate-limit key.
    pub client_ip_header: Option<axum::http::HeaderName>,
}

pub struct AppState {
    pub packs: Vec<RenderedPack>,
    index: HashMap<String, usize>,
    pub base_url: String,
    /// Cookies get the `Secure` attribute when the site is served over https.
    pub secure_cookies: bool,
    pub client_ip_header: Option<axum::http::HeaderName>,
    pub db: Db,
    pub mailer: Mailer,
    pub limiter: RateLimiter,
    /// Verified against when a login names an unknown user, so that case
    /// takes as long as a wrong password.
    pub dummy_hash: String,
    /// Short content hashes used to cache-bust static assets.
    pub css_v: String,
    pub js_v: String,
    pub theme_v: String,
}

impl AppState {
    pub fn new(packs: Vec<Pack>, config: Config, db: Db, mailer: Mailer) -> Self {
        let hl = Highlighter::new();
        let packs: Vec<RenderedPack> = packs
            .into_iter()
            .map(|pack| {
                let files = pack
                    .files
                    .iter()
                    .map(|f| RenderedFile {
                        path: f.path.clone(),
                        size: f.data.len(),
                        inline: f
                            .text()
                            .filter(|_| f.data.len() <= MAX_INLINE_BYTES)
                            .map(|t| (hl.highlight(&f.path, t), line_count(t))),
                    })
                    .collect();
                RenderedPack {
                    brief_html: render_markdown(&pack.brief_md),
                    explainer_html: render_markdown(&pack.explainer_md),
                    files,
                    pack,
                }
            })
            .collect();
        let index = packs
            .iter()
            .enumerate()
            .map(|(i, p)| (p.pack.meta.id.clone(), i))
            .collect();
        Self {
            packs,
            index,
            secure_cookies: config.base_url.starts_with("https://"),
            base_url: config.base_url.trim_end_matches('/').to_owned(),
            client_ip_header: config.client_ip_header,
            db,
            mailer,
            limiter: RateLimiter::default(),
            dummy_hash: hash_password_blocking("necropsy dummy password")
                .expect("argon2 hashing failed"),
            css_v: short_hash(crate::pages::CSS),
            js_v: short_hash(crate::pages::JS),
            theme_v: short_hash(crate::pages::THEME_JS),
        }
    }

    pub fn get(&self, id: &str) -> Option<&RenderedPack> {
        self.index.get(id).map(|&i| &self.packs[i])
    }

    pub fn position(&self, id: &str) -> Option<usize> {
        self.index.get(id).copied()
    }

    /// Today's pack. The packs cycle in order, one per UTC day.
    pub fn today_index(&self) -> usize {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        ((secs / 86_400) % self.packs.len() as u64) as usize
    }
}

fn short_hash(s: &str) -> String {
    let d = Sha256::digest(s.as_bytes());
    d[..5].iter().map(|b| format!("{b:02x}")).collect()
}
