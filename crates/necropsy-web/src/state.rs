// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared, read-only server state: packs rendered once at startup.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use necropsy_pack::{Pack, Track};
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
    /// Peers allowed to supply `client_ip_header`. When empty, any peer is
    /// believed; otherwise the header from every other peer is ignored.
    pub trusted_proxies: Vec<crate::ipnet::IpNet>,
    /// Live puzzles are offered only when this is set.
    pub live: Option<crate::live::LiveConfig>,
    pub captcha: crate::captcha::Captcha,
    /// Log each request's method, path and status. Off unless asked for.
    pub log_requests: bool,
}

impl RenderedPack {
    /// "Intro #2" or "Daily #3".
    pub fn label(&self) -> String {
        let m = &self.pack.meta;
        match m.track {
            Track::Intro => format!("Intro #{}", m.number),
            Track::Daily => format!("Daily #{}", m.number),
            Track::Live => format!("Live #{}", m.number),
        }
    }
}

/// A link to another puzzle, for the previous and next buttons.
pub struct Neighbor {
    pub href: String,
    pub label: String,
}

pub struct AppState {
    /// Every pack, intro stages first and dailies by date. Private: callers go
    /// through the methods below, which hide dailies that are not released yet.
    all: Vec<RenderedPack>,
    index: HashMap<String, usize>,
    /// Treat this day (days since 1970-01-01 UTC) as today instead of reading
    /// the clock. For tests.
    pub today_override: Option<i64>,
    pub base_url: String,
    /// Cookies get the `Secure` attribute when the site is served over https.
    pub secure_cookies: bool,
    pub client_ip_header: Option<axum::http::HeaderName>,
    pub trusted_proxies: Vec<crate::ipnet::IpNet>,
    pub live: Option<crate::live::LiveConfig>,
    pub captcha: crate::captcha::Captcha,
    pub log_requests: bool,
    pub hub: crate::live::LiveHub,
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
        // Live puzzles need the live domain; without it they stay hidden.
        let live_on = config.live.is_some();
        let range_on = config.live.as_ref().is_some_and(|l| l.range.is_some());
        let packs: Vec<RenderedPack> = packs
            .into_iter()
            .filter(|p| live_on || !p.is_live())
            .filter(|p| range_on || p.container().is_none())
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
            all: packs,
            index,
            today_override: None,
            secure_cookies: config.base_url.starts_with("https://"),
            base_url: config.base_url.trim_end_matches('/').to_owned(),
            client_ip_header: config.client_ip_header,
            trusted_proxies: config.trusted_proxies,
            live: config.live,
            captcha: config.captcha,
            log_requests: config.log_requests,
            hub: crate::live::LiveHub::default(),
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

    /// Today as days since 1970-01-01 UTC.
    pub fn today_days(&self) -> i64 {
        self.today_override.unwrap_or_else(|| {
            let secs = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            (secs / 86_400) as i64
        })
    }

    /// Intro stages are always visible. A daily is visible from its release
    /// day on. Every lookup goes through this, so an unreleased puzzle cannot
    /// be read by its page, its files, its answer check or its explainer.
    fn is_visible(&self, p: &RenderedPack) -> bool {
        match p.pack.meta.track {
            Track::Intro | Track::Live => true,
            Track::Daily => p
                .pack
                .meta
                .release_day()
                .is_some_and(|d| d <= self.today_days()),
        }
    }

    /// A visible pack by id.
    pub fn get(&self, id: &str) -> Option<&RenderedPack> {
        self.index
            .get(id)
            .map(|&i| &self.all[i])
            .filter(|p| self.is_visible(p))
    }

    /// Any loaded pack by id, released or not. For the admin, who may start any
    /// live puzzle. Never use it for a player.
    pub fn get_any(&self, id: &str) -> Option<&RenderedPack> {
        self.index.get(id).map(|&i| &self.all[i])
    }

    /// Every loaded live-mode pack, released or not, for the admin.
    pub fn all_live(&self) -> Vec<&RenderedPack> {
        self.all.iter().filter(|p| p.pack.is_live()).collect()
    }

    /// Every live-mode puzzle a player can start now: the always-available live
    /// track, plus dailies that are live once they are released.
    pub fn lives(&self) -> Vec<&RenderedPack> {
        self.all
            .iter()
            .filter(|p| p.pack.is_live() && self.is_visible(p))
            .collect()
    }

    /// Intro stages, in order.
    pub fn intros(&self) -> Vec<&RenderedPack> {
        self.all
            .iter()
            .filter(|p| p.pack.meta.track == Track::Intro)
            .collect()
    }

    /// Released dailies, oldest first.
    pub fn released_dailies(&self) -> Vec<&RenderedPack> {
        self.all
            .iter()
            .filter(|p| p.pack.meta.track == Track::Daily && self.is_visible(p))
            .collect()
    }

    /// The latest released daily, which is today's puzzle when one is dated today.
    pub fn today_daily(&self) -> Option<&RenderedPack> {
        self.released_dailies().pop()
    }

    /// Whether this daily is dated today.
    pub fn is_today(&self, p: &RenderedPack) -> bool {
        p.pack.meta.track == Track::Daily && p.pack.meta.release_day() == Some(self.today_days())
    }

    /// Ids of every pack that can be played right now.
    pub fn visible_ids(&self) -> Vec<String> {
        self.all
            .iter()
            .filter(|p| self.is_visible(p))
            .map(|p| p.pack.meta.id.clone())
            .collect()
    }

    pub fn visible_count(&self) -> usize {
        self.all.iter().filter(|p| self.is_visible(p)).count()
    }

    /// Previous and next puzzle in the same track. The last intro stage leads
    /// on to today's daily.
    pub fn neighbors(&self, p: &RenderedPack) -> (Option<Neighbor>, Option<Neighbor>) {
        let link = |q: &RenderedPack| Neighbor {
            href: format!("/p/{}", q.pack.meta.id),
            label: format!("{} {}", q.label(), q.pack.meta.title),
        };
        let list = match p.pack.meta.track {
            Track::Intro => self.intros(),
            Track::Daily => self.released_dailies(),
            Track::Live => self
                .all
                .iter()
                .filter(|q| q.pack.meta.track == Track::Live)
                .collect(),
        };
        let Some(pos) = list.iter().position(|q| q.pack.meta.id == p.pack.meta.id) else {
            return (None, None);
        };
        let prev = pos.checked_sub(1).map(|i| link(list[i]));
        let next = match list.get(pos + 1) {
            Some(q) => Some(link(q)),
            None if p.pack.meta.track == Track::Intro => self.today_daily().map(|d| Neighbor {
                href: "/".into(),
                label: format!("Today's puzzle: {}", d.pack.meta.title),
            }),
            None => None,
        };
        (prev, next)
    }
}

fn short_hash(s: &str) -> String {
    let d = Sha256::digest(s.as_bytes());
    d[..5].iter().map(|b| format!("{b:02x}")).collect()
}
