// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared, read-only server state: packs rendered once at startup.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use necropsy_pack::{Pack, Track};
use sha2::{Digest, Sha256};

use crate::auth::{hash_password_blocking, RateLimiter};
use crate::db::{Db, PackStats};
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

/// Solve counts and averages by puzzle id.
pub type StatsMap = HashMap<String, PackStats>;

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
            Track::Library => format!("Library #{}", m.number),
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
    /// the clock, for every viewer whatever their timezone. For tests.
    pub today_override: Option<i64>,
    /// Treat this as the current UTC time (seconds since the epoch). For tests of
    /// timezones.
    pub clock_override: Option<i64>,
    /// Solve counts and averages per puzzle, recomputed at most every 30 seconds and
    /// whenever someone solves something.
    stats_cache: Mutex<Option<(Instant, Arc<StatsMap>)>>,
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
    /// The in-browser toolkit's third-party builds, found at startup.
    pub toolkit: crate::toolkit::Toolkit,
    /// The Learn modules, in order.
    pub learn: Vec<crate::learn::LearnModule>,
    /// Discord: the invite link and the discussion threads of closed dailies.
    pub community: crate::community::Community,
    /// Runs generated packs, with the server secret. See `generated.rs`.
    pub generator: crate::generated::Generator,
    /// Sealed packs: the wrap key, each loaded pack's key, and what happened to each.
    pub sealed: crate::sealed::Sealed,
    /// Highlights code files: at startup for packs, per player for generated ones.
    hl: Highlighter,
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
                let files = render_files(&hl, &pack.files);
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
            clock_override: None,
            stats_cache: Mutex::new(None),
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
            toolkit: crate::toolkit::Toolkit::default(),
            learn: Vec::new(),
            community: crate::community::Community::default(),
            generator: crate::generated::Generator::default(),
            sealed: crate::sealed::Sealed::default(),
            hl,
        }
    }

    /// Files as the puzzle page shows them: text highlighted, with line numbers.
    pub fn render_files(&self, files: &[necropsy_pack::PackFile]) -> Vec<RenderedFile> {
        render_files(&self.hl, files)
    }

    /// Sets the Learn modules, rendering their markdown.
    pub fn set_learn(&mut self, modules: Vec<necropsy_pack::learn::Module>) {
        self.learn = modules
            .into_iter()
            .map(crate::learn::LearnModule::new)
            .collect();
    }

    /// Solve counts and averages for every puzzle.
    pub async fn stats(&self) -> Arc<StatsMap> {
        {
            let cache = self.stats_cache.lock().unwrap_or_else(|p| p.into_inner());
            if let Some((at, s)) = cache.as_ref() {
                if at.elapsed() < Duration::from_secs(30) {
                    return s.clone();
                }
            }
        }
        let fresh = Arc::new(self.db.pack_stats().await.unwrap_or_default());
        *self.stats_cache.lock().unwrap_or_else(|p| p.into_inner()) =
            Some((Instant::now(), fresh.clone()));
        fresh
    }

    /// Drops the cached statistics, after something that changes them.
    pub fn forget_stats(&self) {
        *self.stats_cache.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }

    /// Today for the viewer making this request, as a day number (days since
    /// 1970-01-01 of the viewer's calendar). It is the date in the viewer's
    /// timezone, but the timezone is clamped to the real ones (UTC-12 to UTC+14),
    /// so the furthest ahead anyone can get is the date in UTC+14, a few hours
    /// ahead of the server's own date: nobody can read a puzzle days early.
    pub fn today_days(&self) -> i64 {
        if let Some(d) = self.today_override {
            return d;
        }
        let secs = self.clock_override.unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0)
        });
        (secs + client_tz_min() * 60).div_euclid(86_400)
    }

    /// Seconds since the epoch: the clock, or a test's override. With only
    /// `today_override` set, it is noon UTC of that day.
    pub fn now_secs(&self) -> i64 {
        if let Some(s) = self.clock_override {
            return s;
        }
        if let Some(d) = self.today_override {
            return d * 86_400 + 43_200;
        }
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }

    /// Whether a daily has closed: its date is over in every timezone, the last
    /// being UTC-12. Before that it is someone's puzzle of the day, so it is not
    /// discussed in public.
    pub fn is_closed(&self, p: &RenderedPack) -> bool {
        p.pack.meta.track == Track::Daily
            && p.pack
                .meta
                .release_day()
                .is_some_and(|d| self.now_secs() >= (d + 1) * 86_400 - TZ_MIN * 60)
    }

    /// Intro stages are always visible. A daily is visible from its release
    /// day on. Every lookup goes through this, so an unreleased puzzle cannot
    /// be read by its page, its files, its answer check or its explainer.
    fn is_visible(&self, p: &RenderedPack) -> bool {
        match p.pack.meta.track {
            Track::Intro | Track::Library => true,
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

    /// Every puzzle a player can open now, in order.
    pub fn visible_packs(&self) -> Vec<&RenderedPack> {
        self.all.iter().filter(|p| self.is_visible(p)).collect()
    }

    /// The library, in order: puzzles that are not part of the daily rotation.
    /// Always available.
    pub fn library(&self) -> Vec<&RenderedPack> {
        self.all
            .iter()
            .filter(|p| p.pack.meta.track == Track::Library)
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
            Track::Library => self.library(),
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

fn render_files(hl: &Highlighter, files: &[necropsy_pack::PackFile]) -> Vec<RenderedFile> {
    files
        .iter()
        .map(|f| RenderedFile {
            path: f.path.clone(),
            size: f.data.len(),
            inline: f
                .text()
                .filter(|_| f.data.len() <= MAX_INLINE_BYTES)
                .map(|t| (hl.highlight(&f.path, t), line_count(t))),
        })
        .collect()
}

pub fn short_hash(s: &str) -> String {
    let d = Sha256::digest(s.as_bytes());
    d[..5].iter().map(|b| format!("{b:02x}")).collect()
}

// ---------------------------------------------------------------- viewer timezone

/// The furthest-behind and furthest-ahead real UTC offsets, in minutes.
pub const TZ_MIN: i64 = -12 * 60;
pub const TZ_MAX: i64 = 14 * 60;

tokio::task_local! {
    /// The current request's timezone, in minutes ahead of UTC, already clamped.
    static CLIENT_TZ: i64;
}

/// The offset a cookie claims, clamped to the real timezones; 0 (UTC) if it is
/// missing or not a number.
pub fn parse_tz(cookie_header: Option<&str>) -> i64 {
    cookie_header
        .into_iter()
        .flat_map(|h| h.split(';'))
        .filter_map(|c| c.trim().strip_prefix("necropsy_tz="))
        .find_map(|v| v.parse::<i64>().ok())
        .map_or(0, |m| m.clamp(TZ_MIN, TZ_MAX))
}

/// Runs `fut` with this timezone as the viewer's.
pub async fn with_tz<F: std::future::Future>(minutes: i64, fut: F) -> F::Output {
    CLIENT_TZ.scope(minutes, fut).await
}

/// The viewer's timezone for the request being handled, in minutes ahead of UTC.
pub fn client_tz_min() -> i64 {
    CLIENT_TZ.try_with(|m| *m).unwrap_or(0)
}
