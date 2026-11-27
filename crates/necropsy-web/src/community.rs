// SPDX-License-Identifier: AGPL-3.0-or-later
//! Discord: a link to the server, and a discussion thread for each daily once
//! it has closed. See `docs/community.md`.
//!
//! A daily is discussed only after it has closed everywhere (its date is over
//! in UTC-12), so no thread spoils a puzzle that someone can still play as the
//! day's. The site makes the thread itself, through a webhook on a Discord
//! forum channel, and links it from the puzzle page.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;

use crate::state::{AppState, RenderedPack};

/// Threads are made for dailies that closed at most this long ago, so turning
/// the feature on does not post a thread for every old daily.
const BACKFILL_SECS: i64 = 14 * 86_400;
/// Threads made per run, at most. A run is every few minutes.
const PER_RUN: usize = 3;
/// How often the site looks for dailies that need a thread.
pub const INTERVAL: Duration = Duration::from_secs(5 * 60);

/// The forum channel threads are posted to.
pub struct Forum {
    /// The forum channel's webhook URL. It holds a secret token.
    pub webhook: String,
    /// The server's id, for the thread's address.
    pub guild: String,
    client: reqwest::Client,
}

impl Forum {
    pub fn new(webhook: String, guild: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .expect("cannot build the Discord http client");
        Self {
            webhook,
            guild,
            client,
        }
    }
}

#[derive(Default)]
pub struct Community {
    /// The invite link, shown in the footer and on closed dailies.
    pub invite: Option<String>,
    pub forum: Option<Forum>,
    /// Thread address by pack id, loaded at startup and added to as threads are made.
    threads: Mutex<HashMap<String, String>>,
}

impl Community {
    pub fn new(invite: Option<String>, forum: Option<Forum>) -> Self {
        Self {
            invite,
            forum,
            threads: Mutex::default(),
        }
    }

    /// Whether there is anywhere to discuss a puzzle.
    pub fn on(&self) -> bool {
        self.invite.is_some() || self.forum.is_some()
    }

    pub fn thread(&self, pack_id: &str) -> Option<String> {
        self.threads
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(pack_id)
            .cloned()
    }

    fn remember(&self, pack_id: String, url: String) {
        self.threads
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(pack_id, url);
    }

    /// Loads the threads already made.
    pub async fn load(&self, db: &crate::db::Db) {
        match db.discussions().await {
            Ok(map) => *self.threads.lock().unwrap_or_else(|p| p.into_inner()) = map,
            Err(e) => tracing::error!("loading discussion threads failed: {e}"),
        }
    }
}

/// Where a closed daily is discussed: its thread, or else the invite. `None`
/// for a puzzle that is not a closed daily, or when Discord is not set up.
pub fn discussion_link(state: &AppState, rp: &RenderedPack) -> Option<String> {
    if !state.is_closed(rp) {
        return None;
    }
    state
        .community
        .thread(&rp.pack.meta.id)
        .or_else(|| state.community.invite.clone())
}

#[derive(Deserialize)]
struct WebhookReply {
    /// For a message posted to a forum with `thread_name`, the new thread's id.
    channel_id: String,
}

/// The thread's first message.
fn opening_post(state: &AppState, rp: &RenderedPack) -> (String, String) {
    let m = &rp.pack.meta;
    let name: String = format!("{}: {}", rp.label(), m.title)
        .chars()
        .take(100)
        .collect();
    let content = format!(
        "{} has closed. Spoilers are fine in this thread.\n{}/p/{}",
        rp.label(),
        state.base_url,
        m.id
    );
    (name, content)
}

/// Makes a thread for one daily. Returns its address.
async fn post_thread(state: &AppState, forum: &Forum, rp: &RenderedPack) -> Result<String, String> {
    let (name, content) = opening_post(state, rp);
    let sep = if forum.webhook.contains('?') {
        '&'
    } else {
        '?'
    };
    let res = forum
        .client
        .post(format!("{}{sep}wait=true", forum.webhook))
        .json(&serde_json::json!({
            "thread_name": name,
            "content": content,
            // Never ping anyone, whatever a title contains.
            "allowed_mentions": { "parse": [] },
        }))
        .send()
        .await
        .map_err(|e| crate::live::error_chain(&e))?;
    let status = res.status();
    if !status.is_success() {
        let body: String = res
            .text()
            .await
            .unwrap_or_default()
            .chars()
            .take(300)
            .collect();
        return Err(format!("Discord answered {status}: {body}"));
    }
    let reply: WebhookReply = res
        .json()
        .await
        .map_err(|e| format!("Discord's answer has no thread id: {e}"))?;
    if reply.channel_id.is_empty() || !reply.channel_id.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!(
            "Discord's thread id {:?} is not an id",
            reply.channel_id
        ));
    }
    Ok(format!(
        "https://discord.com/channels/{}/{}",
        forum.guild, reply.channel_id
    ))
}

/// Makes the threads that are due: dailies that closed in the last 14 days and
/// have none. A failure is logged and tried again on the next run.
pub async fn post_due_threads(state: &Arc<AppState>) -> usize {
    let Some(forum) = &state.community.forum else {
        return 0;
    };
    let now = state.now_secs();
    let due: Vec<&RenderedPack> = state
        .released_dailies()
        .into_iter()
        .filter(|p| state.is_closed(p) && state.community.thread(&p.pack.meta.id).is_none())
        .filter(|p| {
            p.pack.meta.release_day().is_some_and(|d| {
                let closed_at = (d + 1) * 86_400 - crate::state::TZ_MIN * 60;
                now - closed_at <= BACKFILL_SECS
            })
        })
        .take(PER_RUN)
        .collect();
    let mut made = 0;
    for rp in due {
        let id = rp.pack.meta.id.clone();
        match post_thread(state, forum, rp).await {
            Ok(url) => {
                if let Err(e) = state.db.add_discussion(id.clone(), url.clone()).await {
                    tracing::error!("saving the discussion thread of {id} failed: {e}");
                }
                tracing::info!("made the discussion thread for {id}: {url}");
                state.community.remember(id, url);
                made += 1;
            }
            Err(e) => {
                tracing::warn!("could not make the discussion thread for {id}: {e}");
                break;
            }
        }
    }
    made
}

/// Checks the settings: an `https://` invite, and a webhook with its server id.
pub fn from_env() -> Result<Community, String> {
    from_vars(|k| std::env::var(k).ok())
}

/// [`from_env`], reading settings through `var`.
pub fn from_vars(var: impl Fn(&str) -> Option<String>) -> Result<Community, String> {
    let get = |k: &str| {
        var(k)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    let invite = get("NECROPSY_DISCORD_URL");
    if let Some(u) = &invite {
        if !u.starts_with("https://") || u.contains(char::is_whitespace) {
            return Err(format!(
                "NECROPSY_DISCORD_URL {u:?} must be an https:// link"
            ));
        }
    }
    let forum = match (
        get("NECROPSY_DISCORD_WEBHOOK"),
        get("NECROPSY_DISCORD_GUILD_ID"),
    ) {
        (Some(w), Some(g)) => {
            if !(w.starts_with("https://") || w.starts_with("http://")) {
                return Err("NECROPSY_DISCORD_WEBHOOK must be the webhook's URL".into());
            }
            if g.is_empty() || !g.bytes().all(|b| b.is_ascii_digit()) {
                return Err(format!(
                    "NECROPSY_DISCORD_GUILD_ID {g:?} must be the server's numeric id"
                ));
            }
            Some(Forum::new(w, g))
        }
        (None, None) => None,
        _ => {
            return Err(
                "set both NECROPSY_DISCORD_WEBHOOK and NECROPSY_DISCORD_GUILD_ID, or neither"
                    .into(),
            )
        }
    };
    Ok(Community::new(invite, forum))
}
