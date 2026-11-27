// SPDX-License-Identifier: AGPL-3.0-or-later
//! SQLite storage for accounts, sessions, email tokens and solves.
//!
//! One connection behind a mutex, used from `spawn_blocking`. That is plenty
//! for this site and keeps the code simple. The schema is created on startup;
//! `PRAGMA user_version` records its version for future migrations.
//!
//! Email addresses are stored but never read back out for display. [`User`]
//! deliberately has no email field, so no page or API can render one by
//! accident. The address is only read by [`Db::email_of`], which the mailer
//! path uses for security notices.

use std::sync::{Arc, Mutex};

use rusqlite::{params, Connection, ErrorCode, OptionalExtension};

const SCHEMA_VERSION: i64 = 9;

/// Generated packs (version 9): each player's random seed per pack, and which
/// live instances have flags keyed with the server secret (all made from now
/// on; older ones keep the flag they were made with). See docs/generators.md.
const GEN_SCHEMA: &str = "
CREATE TABLE gen_seeds (
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    pack_id    TEXT NOT NULL,
    seed       TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, pack_id)
);
ALTER TABLE live_instances ADD COLUMN keyed INTEGER NOT NULL DEFAULT 0;
";

/// Discussion threads for closed dailies (version 8): the Discord thread the
/// site made for each. See docs/community.md.
const DISCUSSION_SCHEMA: &str = "
CREATE TABLE discussions (
    pack_id    TEXT PRIMARY KEY,
    url        TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
";

/// The Learn track (version 7): checkpoints passed, and the tool path a player
/// chose. See docs/learn.md.
const LEARN_SCHEMA: &str = "
CREATE TABLE learn_passed (
    user_id   INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    module    TEXT NOT NULL,
    passed_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, module)
);
ALTER TABLE users ADD COLUMN learn_path TEXT;
";

/// Two-step verification (version 4): an authenticator-app secret and the last
/// time step accepted from it, email codes, one-time recovery codes, the short-lived
/// challenge a password login leaves behind, and an enrollment in progress.
const MFA_SCHEMA: &str = "
ALTER TABLE users ADD COLUMN totp_secret TEXT;
ALTER TABLE users ADD COLUMN totp_last_step INTEGER NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN mfa_email INTEGER NOT NULL DEFAULT 0;
CREATE TABLE recovery_codes (
    user_id   INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    code_hash TEXT NOT NULL,
    PRIMARY KEY (user_id, code_hash)
);
CREATE TABLE mfa_tickets (
    token_hash    TEXT PRIMARY KEY,
    user_id       INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    expires_at    INTEGER NOT NULL,
    attempts      INTEGER NOT NULL DEFAULT 0,
    email_hash    TEXT,
    email_expires INTEGER NOT NULL DEFAULT 0,
    email_sent_at INTEGER NOT NULL DEFAULT 0,
    email_sends   INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE mfa_setup (
    user_id    INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    secret     TEXT NOT NULL,
    expires_at INTEGER NOT NULL
);
";

/// Server-side progress (version 5): when a signed-in player first opened a puzzle and
/// the hints they have revealed, so neither can be reported by the browser. The time
/// is the wall-clock time since `started_at`, so closing the tab does not pause it.
/// `started_at = 0` means the clock has not started yet, as on a live puzzle before the
/// player starts an instance.
/// `solves.timed` is 1 when a solve was recorded from this, 0 when the time and hints
/// were only the browser's word (an import of a solve made while signed out).
const PROGRESS_SCHEMA: &str = "
CREATE TABLE progress (
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    pack_id    TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    hints      INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (user_id, pack_id)
);
ALTER TABLE solves ADD COLUMN timed INTEGER NOT NULL DEFAULT 0;
CREATE INDEX solves_pack ON solves(pack_id);
";

/// Solve cards (version 6): a solve the player chose to share gets a random token, and
/// the card page is looked up by it. NULL means not shared.
const SHARE_SCHEMA: &str = "
ALTER TABLE solves ADD COLUMN share TEXT;
CREATE UNIQUE INDEX solves_share ON solves(share);
";

const SCHEMA: &str = "
CREATE TABLE users (
    id            INTEGER PRIMARY KEY,
    username      TEXT NOT NULL UNIQUE COLLATE NOCASE,
    email         TEXT UNIQUE,            -- normalized; NULL for the admin
    password_hash TEXT NOT NULL,
    is_admin      INTEGER NOT NULL DEFAULT 0,
    verified      INTEGER NOT NULL DEFAULT 0,
    disabled      INTEGER NOT NULL DEFAULT 0,
    created_at    INTEGER NOT NULL
);
CREATE TABLE sessions (
    token_hash TEXT PRIMARY KEY,          -- sha256 of the cookie value
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    expires_at INTEGER NOT NULL
);
CREATE INDEX sessions_user ON sessions(user_id);
CREATE TABLE email_tokens (
    token_hash TEXT PRIMARY KEY,          -- sha256 of the emailed token
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    purpose    TEXT NOT NULL,             -- verify | reset | change_email
    new_email  TEXT,                      -- only for change_email
    expires_at INTEGER NOT NULL
);
CREATE INDEX email_tokens_user ON email_tokens(user_id, purpose);
CREATE TABLE solves (
    user_id   INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    pack_id   TEXT NOT NULL,
    solved_at INTEGER NOT NULL,
    seconds   INTEGER NOT NULL,
    hints     INTEGER NOT NULL,
    tz        INTEGER NOT NULL DEFAULT 0,  -- minutes ahead of UTC when it was solved
    PRIMARY KEY (user_id, pack_id)
);
";

/// Added in schema version 2: live instances, the per-player servers of live
/// puzzles. `label` is the instance's subdomain; `seed` makes its flag and
/// derived values unique to it.
const LIVE_SCHEMA: &str = "
CREATE TABLE live_instances (
    label      TEXT PRIMARY KEY,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    pack_id    TEXT NOT NULL,
    seed       TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    flags      TEXT NOT NULL DEFAULT ''   -- comma-separated scenario state flags
);
CREATE INDEX live_instances_user ON live_instances(user_id, pack_id);
";

/// Expired instances are kept this long, so a flag found just as an instance
/// ended can still be submitted.
pub const LIVE_KEEP_SECS: i64 = 7 * 86_400;

/// Unverified accounts are deleted after this long, freeing the username.
pub const UNVERIFIED_TTL_SECS: i64 = 48 * 3600;

#[derive(Clone, Debug)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub is_admin: bool,
    pub verified: bool,
    pub disabled: bool,
    pub created_at: i64,
    /// An authenticator app is set up.
    pub totp: bool,
    /// Login codes are sent to the account's email.
    pub mfa_email: bool,
}

impl User {
    /// Whether a password login must be followed by a second step.
    pub fn mfa(&self) -> bool {
        self.totp || self.mfa_email
    }
}

/// A signed-in player's time and hints on a puzzle, as the server keeps them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub seconds: i64,
    pub hints: i64,
    /// Whether the clock has started. A live puzzle's starts with its instance.
    pub running: bool,
}

/// What players have done on a puzzle, in aggregate.
#[derive(Clone, Debug)]
pub struct PackStats {
    /// Everyone who solved it.
    pub solves: i64,
    /// Those whose time the server kept; the averages are over these.
    pub timed: i64,
    pub avg_seconds: Option<f64>,
    pub avg_hints: Option<f64>,
}

/// A password login waiting for its second step.
#[derive(Debug)]
pub struct MfaTicket {
    pub user_id: i64,
    pub expires_at: i64,
    pub attempts: i64,
    pub email_hash: Option<String>,
    pub email_expires: i64,
    pub email_sent_at: i64,
    pub email_sends: i64,
}

#[derive(Clone, Debug)]
pub struct Instance {
    pub label: String,
    pub user_id: i64,
    pub pack_id: String,
    pub seed: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub flags: Vec<String>,
    /// The flag is keyed with the server secret (see `live::flag_of`).
    pub keyed: bool,
}

const INSTANCE_COLS: &str = "label, user_id, pack_id, seed, created_at, expires_at, flags, keyed";

fn row_instance(r: &rusqlite::Row<'_>) -> rusqlite::Result<Instance> {
    let flags: String = r.get(6)?;
    Ok(Instance {
        label: r.get(0)?,
        user_id: r.get(1)?,
        pack_id: r.get(2)?,
        seed: r.get(3)?,
        created_at: r.get(4)?,
        expires_at: r.get(5)?,
        flags: flags
            .split(',')
            .filter(|f| !f.is_empty())
            .map(str::to_owned)
            .collect(),
        keyed: r.get(7)?,
    })
}

#[derive(Debug)]
pub struct Solve {
    pub pack_id: String,
    pub solved_at: i64,
    pub seconds: i64,
    pub hints: i64,
    /// The solver's timezone when they solved it, in minutes ahead of UTC.
    pub tz: i64,
    /// The server kept this solve's time and hints (see `progress`).
    pub timed: bool,
    /// The token of the solve's share card, if the solver made one.
    pub share: Option<String>,
}

/// A solve looked up by its share token, for the card page.
#[derive(Debug)]
pub struct SharedSolve {
    pub user_id: i64,
    pub pack_id: String,
    pub seconds: i64,
    pub hints: i64,
    pub solved_at: i64,
    pub tz: i64,
}

#[derive(Debug)]
pub struct AdminRow {
    pub user: User,
    pub solves: i64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CreateError {
    UsernameTaken,
    EmailTaken,
    Other(String),
}

#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn row_user(r: &rusqlite::Row<'_>) -> rusqlite::Result<User> {
    Ok(User {
        id: r.get(0)?,
        username: r.get(1)?,
        is_admin: r.get(2)?,
        verified: r.get(3)?,
        disabled: r.get(4)?,
        created_at: r.get(5)?,
        totp: r.get(6)?,
        mfa_email: r.get(7)?,
    })
}

const USER_COLS: &str =
    "id, username, is_admin, verified, disabled, created_at, totp_secret IS NOT NULL, mfa_email";

impl Db {
    /// Opens (creating if needed) the database at `path`; `:memory:` works.
    pub fn open(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if !(0..=SCHEMA_VERSION).contains(&version) {
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "database schema version {version} is not supported (expected {SCHEMA_VERSION})"
            )));
        }
        if version == 0 {
            conn.execute_batch(SCHEMA)?;
            conn.execute_batch(LIVE_SCHEMA)?;
        } else {
            if version == 1 {
                conn.execute_batch(LIVE_SCHEMA)?;
            }
            if version <= 2 {
                conn.execute_batch("ALTER TABLE solves ADD COLUMN tz INTEGER NOT NULL DEFAULT 0;")?;
            }
        }
        if version < 4 {
            conn.execute_batch(MFA_SCHEMA)?;
        }
        if version < 5 {
            conn.execute_batch(PROGRESS_SCHEMA)?;
        }
        if version < 6 {
            conn.execute_batch(SHARE_SCHEMA)?;
        }
        if version < 7 {
            conn.execute_batch(LEARN_SCHEMA)?;
        }
        if version < 8 {
            conn.execute_batch(DISCUSSION_SCHEMA)?;
        }
        if version < 9 {
            conn.execute_batch(GEN_SCHEMA)?;
        }
        if version != SCHEMA_VERSION {
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Runs `f` on the connection off the async runtime.
    pub async fn run<T, F>(&self, f: F) -> rusqlite::Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> rusqlite::Result<T> + Send + 'static,
    {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
            f(&mut guard)
        })
        .await
        .map_err(|_| rusqlite::Error::InvalidQuery)?
    }

    // ---- users -------------------------------------------------------------

    pub async fn has_admin(&self) -> rusqlite::Result<bool> {
        self.run(|c| {
            c.query_row(
                "SELECT EXISTS(SELECT 1 FROM users WHERE is_admin = 1)",
                [],
                |r| r.get(0),
            )
        })
        .await
    }

    pub async fn create_user(
        &self,
        username: String,
        email: Option<String>,
        password_hash: String,
        is_admin: bool,
        verified: bool,
    ) -> Result<i64, CreateError> {
        let res = self
            .run(move |c| {
                c.execute(
                    "INSERT INTO users (username, email, password_hash, is_admin, verified, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![username, email, password_hash, is_admin, verified, now()],
                )?;
                Ok(c.last_insert_rowid())
            })
            .await;
        res.map_err(classify_unique)
    }

    pub async fn user_by_id(&self, id: i64) -> rusqlite::Result<Option<User>> {
        self.run(move |c| {
            c.query_row(
                &format!("SELECT {USER_COLS} FROM users WHERE id = ?1"),
                [id],
                row_user,
            )
            .optional()
        })
        .await
    }

    /// The user and their password hash, for login.
    pub async fn user_for_login(
        &self,
        username: String,
    ) -> rusqlite::Result<Option<(User, String)>> {
        self.run(move |c| {
            c.query_row(
                &format!("SELECT {USER_COLS}, password_hash FROM users WHERE username = ?1"),
                [username],
                |r| Ok((row_user(r)?, r.get(8)?)),
            )
            .optional()
        })
        .await
    }

    pub async fn user_id_by_email(&self, email: String) -> rusqlite::Result<Option<i64>> {
        self.run(move |c| {
            c.query_row("SELECT id FROM users WHERE email = ?1", [email], |r| {
                r.get(0)
            })
            .optional()
        })
        .await
    }

    /// The stored address, for system-sent security notices only. Never
    /// pass the result to a page or API response.
    pub async fn email_of(&self, id: i64) -> rusqlite::Result<Option<String>> {
        self.run(move |c| {
            c.query_row("SELECT email FROM users WHERE id = ?1", [id], |r| r.get(0))
                .optional()
                .map(Option::flatten)
        })
        .await
    }

    pub async fn password_hash_of(&self, id: i64) -> rusqlite::Result<Option<String>> {
        self.run(move |c| {
            c.query_row("SELECT password_hash FROM users WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .optional()
        })
        .await
    }

    pub async fn set_verified(&self, id: i64) -> rusqlite::Result<()> {
        self.run(move |c| {
            c.execute("UPDATE users SET verified = 1 WHERE id = ?1", [id])
                .map(|_| ())
        })
        .await
    }

    /// Sets a new password and signs the user out everywhere.
    pub async fn set_password(&self, id: i64, hash: String) -> rusqlite::Result<()> {
        self.run(move |c| {
            let tx = c.transaction()?;
            tx.execute(
                "UPDATE users SET password_hash = ?1 WHERE id = ?2",
                params![hash, id],
            )?;
            tx.execute("DELETE FROM sessions WHERE user_id = ?1", [id])?;
            tx.commit()
        })
        .await
    }

    pub async fn set_email(&self, id: i64, email: String) -> Result<(), CreateError> {
        self.run(move |c| {
            c.execute(
                "UPDATE users SET email = ?1 WHERE id = ?2",
                params![email, id],
            )
            .map(|_| ())
        })
        .await
        .map_err(classify_unique)
    }

    pub async fn set_username(&self, id: i64, username: String) -> Result<(), CreateError> {
        self.run(move |c| {
            c.execute(
                "UPDATE users SET username = ?1 WHERE id = ?2",
                params![username, id],
            )
            .map(|_| ())
        })
        .await
        .map_err(classify_unique)
    }

    pub async fn set_disabled(&self, id: i64, disabled: bool) -> rusqlite::Result<()> {
        self.run(move |c| {
            let tx = c.transaction()?;
            tx.execute(
                "UPDATE users SET disabled = ?1 WHERE id = ?2",
                params![disabled, id],
            )?;
            if disabled {
                tx.execute("DELETE FROM sessions WHERE user_id = ?1", [id])?;
            }
            tx.commit()
        })
        .await
    }

    pub async fn delete_user(&self, id: i64) -> rusqlite::Result<()> {
        self.run(move |c| {
            c.execute("DELETE FROM users WHERE id = ?1", [id])
                .map(|_| ())
        })
        .await
    }

    pub async fn list_users(&self) -> rusqlite::Result<Vec<AdminRow>> {
        self.run(|c| {
            let mut st = c.prepare(
                "SELECT u.id, u.username, u.is_admin, u.verified, u.disabled, u.created_at,
                        u.totp_secret IS NOT NULL, u.mfa_email,
                        (SELECT COUNT(*) FROM solves s WHERE s.user_id = u.id)
                 FROM users u ORDER BY u.created_at DESC, u.id DESC",
            )?;
            let rows = st.query_map([], |r| {
                Ok(AdminRow {
                    user: row_user(r)?,
                    solves: r.get(8)?,
                })
            })?;
            rows.collect()
        })
        .await
    }

    // ---- sessions ----------------------------------------------------------

    pub async fn create_session(
        &self,
        user_id: i64,
        token_hash: String,
        expires_at: i64,
    ) -> rusqlite::Result<()> {
        self.run(move |c| {
            c.execute(
                "INSERT INTO sessions (token_hash, user_id, expires_at) VALUES (?1, ?2, ?3)",
                params![token_hash, user_id, expires_at],
            )
            .map(|_| ())
        })
        .await
    }

    /// The user for a live session. Disabled users have no sessions, but the
    /// check is repeated here so a race can never let one through.
    pub async fn session_user(&self, token_hash: String) -> rusqlite::Result<Option<User>> {
        self.run(move |c| {
            c.query_row(
                "SELECT u.id, u.username, u.is_admin, u.verified, u.disabled, u.created_at,
                        u.totp_secret IS NOT NULL, u.mfa_email
                 FROM sessions s JOIN users u ON u.id = s.user_id
                 WHERE s.token_hash = ?1 AND s.expires_at > ?2 AND u.disabled = 0",
                params![token_hash, now()],
                row_user,
            )
            .optional()
        })
        .await
    }

    pub async fn delete_session(&self, token_hash: String) -> rusqlite::Result<()> {
        self.run(move |c| {
            c.execute("DELETE FROM sessions WHERE token_hash = ?1", [token_hash])
                .map(|_| ())
        })
        .await
    }

    // ---- email tokens ------------------------------------------------------

    /// Stores a token, replacing any earlier one of the same purpose for the user.
    pub async fn put_token(
        &self,
        user_id: i64,
        token_hash: String,
        purpose: &'static str,
        new_email: Option<String>,
        expires_at: i64,
    ) -> rusqlite::Result<()> {
        self.run(move |c| {
            let tx = c.transaction()?;
            tx.execute(
                "DELETE FROM email_tokens WHERE user_id = ?1 AND purpose = ?2",
                params![user_id, purpose],
            )?;
            tx.execute(
                "INSERT INTO email_tokens (token_hash, user_id, purpose, new_email, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![token_hash, user_id, purpose, new_email, expires_at],
            )?;
            tx.commit()
        })
        .await
    }

    /// Looks up and deletes a token in one step, so it works once.
    pub async fn take_token(
        &self,
        token_hash: String,
        purpose: &'static str,
    ) -> rusqlite::Result<Option<(i64, Option<String>)>> {
        self.run(move |c| {
            let tx = c.transaction()?;
            let row = tx
                .query_row(
                    "SELECT user_id, new_email FROM email_tokens
                     WHERE token_hash = ?1 AND purpose = ?2 AND expires_at > ?3",
                    params![token_hash, purpose, now()],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if row.is_some() {
                tx.execute(
                    "DELETE FROM email_tokens WHERE token_hash = ?1",
                    [token_hash],
                )?;
            }
            tx.commit()?;
            Ok(row)
        })
        .await
    }

    /// Whether a token exists and is live, without consuming it.
    pub async fn token_is_live(
        &self,
        token_hash: String,
        purpose: &'static str,
    ) -> rusqlite::Result<bool> {
        self.run(move |c| {
            c.query_row(
                "SELECT EXISTS(SELECT 1 FROM email_tokens
                 WHERE token_hash = ?1 AND purpose = ?2 AND expires_at > ?3)",
                params![token_hash, purpose, now()],
                |r| r.get(0),
            )
        })
        .await
    }

    // ---- solves ------------------------------------------------------------

    /// Records a first solve. Later solves of the same pack change nothing. `timed`
    /// says the time and hints come from the server's own progress record.
    pub async fn record_solve(
        &self,
        user_id: i64,
        pack_id: String,
        seconds: i64,
        hints: i64,
        tz: i64,
        timed: bool,
    ) -> rusqlite::Result<()> {
        self.run(move |c| {
            c.execute(
                "INSERT INTO solves (user_id, pack_id, solved_at, seconds, hints, tz, timed)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) ON CONFLICT DO NOTHING",
                params![user_id, pack_id, now(), seconds, hints, tz, timed],
            )
            .map(|_| ())
        })
        .await
    }

    /// The user's solve of one puzzle, if they have one.
    pub async fn solve_of(&self, user_id: i64, pack_id: String) -> rusqlite::Result<Option<Solve>> {
        self.run(move |c| {
            c.query_row(
                "SELECT pack_id, solved_at, seconds, hints, tz, timed, share FROM solves WHERE user_id = ?1 AND pack_id = ?2",
                params![user_id, pack_id],
                |r| {
                    Ok(Solve {
                        pack_id: r.get(0)?,
                        solved_at: r.get(1)?,
                        seconds: r.get(2)?,
                        hints: r.get(3)?,
                        tz: r.get(4)?,
                        timed: r.get(5)?,
                        share: r.get(6)?,
                    })
                },
            )
            .optional()
        })
        .await
    }

    // ---- progress ----------------------------------------------------------

    /// Notes that the player is on this puzzle and returns their progress. With `start`
    /// the first call starts their clock; without it (a live puzzle, whose clock starts
    /// with the instance) the row is made with the clock stopped. The time is what has
    /// passed since the clock started, up to `cap` seconds: it keeps running while the
    /// page is closed or the machine sleeps, and stops growing at the cap.
    pub async fn progress_beat(
        &self,
        user_id: i64,
        pack_id: String,
        cap: i64,
        start: bool,
    ) -> rusqlite::Result<Progress> {
        self.run(move |c| {
            let t = now();
            c.execute(
                "INSERT OR IGNORE INTO progress (user_id, pack_id, started_at, hints) VALUES (?1, ?2, ?3, 0)",
                params![user_id, pack_id, if start { t } else { 0 }],
            )?;
            let (started, hints): (i64, i64) = c.query_row(
                "SELECT started_at, hints FROM progress WHERE user_id = ?1 AND pack_id = ?2",
                params![user_id, pack_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            Ok(Progress {
                seconds: if started == 0 { 0 } else { (t - started).clamp(0, cap) },
                hints,
                running: started != 0,
            })
        })
        .await
    }

    /// Starts the clock on a puzzle whose clock starts with an action, such as starting
    /// an instance. Starting again changes nothing, so restarting an instance does not
    /// reset the time.
    pub async fn progress_start(&self, user_id: i64, pack_id: String) -> rusqlite::Result<()> {
        self.run(move |c| {
            let t = now();
            c.execute(
                "INSERT OR IGNORE INTO progress (user_id, pack_id, started_at, hints) VALUES (?1, ?2, ?3, 0)",
                params![user_id, pack_id, t],
            )?;
            c.execute(
                "UPDATE progress SET started_at = ?3 WHERE user_id = ?1 AND pack_id = ?2 AND started_at = 0",
                params![user_id, pack_id, t],
            )
            .map(|_| ())
        })
        .await
    }

    /// Reveals the next hint (up to `total`) and returns the progress after it.
    pub async fn progress_hint(
        &self,
        user_id: i64,
        pack_id: String,
        total: i64,
        cap: i64,
        start: bool,
    ) -> rusqlite::Result<Progress> {
        let p = self
            .progress_beat(user_id, pack_id.clone(), cap, start)
            .await?;
        if p.hints >= total {
            return Ok(p);
        }
        self.run(move |c| {
            c.execute(
                "UPDATE progress SET hints = hints + 1 WHERE user_id = ?1 AND pack_id = ?2 AND hints < ?3",
                params![user_id, pack_id, total],
            )?;
            Ok(Progress {
                hints: (p.hints + 1).min(total),
                ..p
            })
        })
        .await
    }

    /// The final progress for a solve, which removes the record: the time since the
    /// clock started (up to `cap`) and the hints revealed. None if there was no record
    /// or its clock never started.
    pub async fn progress_finish(
        &self,
        user_id: i64,
        pack_id: String,
        cap: i64,
    ) -> rusqlite::Result<Option<Progress>> {
        self.run(move |c| {
            let tx = c.transaction()?;
            let row = tx
                .query_row(
                    "SELECT started_at, hints FROM progress WHERE user_id = ?1 AND pack_id = ?2",
                    params![user_id, pack_id],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
                )
                .optional()?;
            let out = row
                .filter(|(started, _)| *started != 0)
                .map(|(started, hints)| Progress {
                    seconds: (now() - started).clamp(0, cap),
                    hints,
                    running: true,
                });
            tx.execute(
                "DELETE FROM progress WHERE user_id = ?1 AND pack_id = ?2",
                params![user_id, pack_id],
            )?;
            tx.commit()?;
            Ok(out)
        })
        .await
    }

    /// Solve counts and averages for every puzzle, from solves whose time was kept by
    /// the server. Aggregates only.
    pub async fn pack_stats(
        &self,
    ) -> rusqlite::Result<std::collections::HashMap<String, PackStats>> {
        self.run(|c| {
            let mut st = c.prepare(
                "SELECT pack_id, COUNT(*), SUM(timed),
                        AVG(CASE WHEN timed = 1 THEN seconds END),
                        AVG(CASE WHEN timed = 1 THEN hints END)
                 FROM solves GROUP BY pack_id",
            )?;
            let rows = st.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    PackStats {
                        solves: r.get(1)?,
                        timed: r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                        avg_seconds: r.get::<_, Option<f64>>(3)?,
                        avg_hints: r.get::<_, Option<f64>>(4)?,
                    },
                ))
            })?;
            rows.collect()
        })
        .await
    }

    /// Learn modules whose checkpoint the user has passed.
    pub async fn learn_passed(&self, user_id: i64) -> rusqlite::Result<Vec<String>> {
        self.run(move |c| {
            let mut st =
                c.prepare("SELECT module FROM learn_passed WHERE user_id = ?1 ORDER BY passed_at")?;
            let rows = st.query_map([user_id], |r| r.get(0))?;
            rows.collect()
        })
        .await
    }

    /// Records a passed checkpoint. Passing again keeps the first time.
    pub async fn learn_pass(&self, user_id: i64, module: String) -> rusqlite::Result<()> {
        self.run(move |c| {
            c.execute(
                "INSERT OR IGNORE INTO learn_passed (user_id, module, passed_at) VALUES (?1, ?2, ?3)",
                params![user_id, module, now()],
            )
            .map(|_| ())
        })
        .await
    }

    /// The tool path the user chose for Learn, `browser` or `terminal`.
    pub async fn learn_path(&self, user_id: i64) -> rusqlite::Result<Option<String>> {
        self.run(move |c| {
            c.query_row(
                "SELECT learn_path FROM users WHERE id = ?1",
                [user_id],
                |r| r.get(0),
            )
            .optional()
            .map(Option::flatten)
        })
        .await
    }

    pub async fn set_learn_path(&self, user_id: i64, path: String) -> rusqlite::Result<()> {
        self.run(move |c| {
            c.execute(
                "UPDATE users SET learn_path = ?2 WHERE id = ?1",
                params![user_id, path],
            )
            .map(|_| ())
        })
        .await
    }

    pub async fn has_solved(&self, user_id: i64, pack_id: String) -> rusqlite::Result<bool> {
        self.run(move |c| {
            c.query_row(
                "SELECT EXISTS(SELECT 1 FROM solves WHERE user_id = ?1 AND pack_id = ?2)",
                params![user_id, pack_id],
                |r| r.get(0),
            )
        })
        .await
    }

    pub async fn solves_of(&self, user_id: i64) -> rusqlite::Result<Vec<Solve>> {
        self.run(move |c| {
            let mut st = c.prepare(
                "SELECT pack_id, solved_at, seconds, hints, tz, timed, share FROM solves WHERE user_id = ?1 ORDER BY solved_at",
            )?;
            let rows = st.query_map([user_id], |r| {
                Ok(Solve {
                    pack_id: r.get(0)?,
                    solved_at: r.get(1)?,
                    seconds: r.get(2)?,
                    hints: r.get(3)?,
                    tz: r.get(4)?,
                    timed: r.get(5)?,
                    share: r.get(6)?,
                })
            })?;
            rows.collect()
        })
        .await
    }

    // ---- solve cards -------------------------------------------------------

    /// The solve's share token, made from `candidate` if it has none. Only a solve
    /// whose time the server kept can be shared: its numbers are the server's, so the
    /// card cannot say something the player made up. None if there is no such solve.
    pub async fn share_enable(
        &self,
        user_id: i64,
        pack_id: String,
        candidate: String,
    ) -> rusqlite::Result<Option<String>> {
        self.run(move |c| {
            c.execute(
                "UPDATE solves SET share = COALESCE(share, ?3) WHERE user_id = ?1 AND pack_id = ?2 AND timed = 1",
                params![user_id, pack_id, candidate],
            )?;
            c.query_row(
                "SELECT share FROM solves WHERE user_id = ?1 AND pack_id = ?2 AND timed = 1",
                params![user_id, pack_id],
                |r| r.get(0),
            )
            .optional()
            .map(Option::flatten)
        })
        .await
    }

    /// Stops sharing a solve: its card page and image stop working.
    pub async fn share_disable(&self, user_id: i64, pack_id: String) -> rusqlite::Result<()> {
        self.run(move |c| {
            c.execute(
                "UPDATE solves SET share = NULL WHERE user_id = ?1 AND pack_id = ?2",
                params![user_id, pack_id],
            )
            .map(|_| ())
        })
        .await
    }

    pub async fn shared_solve(&self, token: String) -> rusqlite::Result<Option<SharedSolve>> {
        self.run(move |c| {
            c.query_row(
                "SELECT pack_id, seconds, hints, solved_at, tz, user_id FROM solves WHERE share = ?1",
                [token],
                |r| {
                    Ok(SharedSolve {
                        pack_id: r.get(0)?,
                        seconds: r.get(1)?,
                        hints: r.get(2)?,
                        solved_at: r.get(3)?,
                        tz: r.get(4)?,
                        user_id: r.get(5)?,
                    })
                },
            )
            .optional()
        })
        .await
    }

    /// The player's generator seed for a pack, if they have opened it.
    pub async fn gen_seed(
        &self,
        user_id: i64,
        pack_id: String,
    ) -> rusqlite::Result<Option<String>> {
        self.run(move |c| {
            c.query_row(
                "SELECT seed FROM gen_seeds WHERE user_id = ?1 AND pack_id = ?2",
                params![user_id, pack_id],
                |r| r.get(0),
            )
            .optional()
        })
        .await
    }

    /// The player's seed for a pack, stored as `initial` the first time.
    pub async fn gen_seed_get_or_create(
        &self,
        user_id: i64,
        pack_id: String,
        initial: String,
    ) -> rusqlite::Result<String> {
        self.run(move |c| {
            c.execute(
                "INSERT OR IGNORE INTO gen_seeds (user_id, pack_id, seed, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![user_id, pack_id, initial, now()],
            )?;
            c.query_row(
                "SELECT seed FROM gen_seeds WHERE user_id = ?1 AND pack_id = ?2",
                params![user_id, pack_id],
                |r| r.get(0),
            )
        })
        .await
    }

    /// The discussion thread of each daily that has one.
    pub async fn discussions(&self) -> rusqlite::Result<std::collections::HashMap<String, String>> {
        self.run(|c| {
            let mut st = c.prepare("SELECT pack_id, url FROM discussions")?;
            let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect()
        })
        .await
    }

    pub async fn add_discussion(&self, pack_id: String, url: String) -> rusqlite::Result<()> {
        self.run(move |c| {
            c.execute(
                "INSERT OR IGNORE INTO discussions (pack_id, url, created_at) VALUES (?1, ?2, ?3)",
                params![pack_id, url, now()],
            )
            .map(|_| ())
        })
        .await
    }

    /// For a card: how many timed solves of the puzzle there are, and how many of them
    /// took longer than `seconds`.
    pub async fn slower_than(&self, pack_id: String, seconds: i64) -> rusqlite::Result<(i64, i64)> {
        self.run(move |c| {
            c.query_row(
                "SELECT COUNT(*), COALESCE(SUM(seconds > ?2), 0) FROM solves WHERE pack_id = ?1 AND timed = 1",
                params![pack_id, seconds],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
        })
        .await
    }

    // ---- two-step verification ---------------------------------------------

    /// Starts a login challenge, replacing the user's earlier one and dropping any
    /// that have expired.
    pub async fn mfa_ticket_put(
        &self,
        token_hash: String,
        user_id: i64,
        expires_at: i64,
    ) -> rusqlite::Result<()> {
        self.run(move |c| {
            let tx = c.transaction()?;
            tx.execute(
                "DELETE FROM mfa_tickets WHERE user_id = ?1 OR expires_at <= ?2",
                params![user_id, now()],
            )?;
            tx.execute(
                "INSERT INTO mfa_tickets (token_hash, user_id, expires_at) VALUES (?1, ?2, ?3)",
                params![token_hash, user_id, expires_at],
            )?;
            tx.commit()
        })
        .await
    }

    pub async fn mfa_ticket_get(&self, token_hash: String) -> rusqlite::Result<Option<MfaTicket>> {
        self.run(move |c| {
            c.query_row(
                "SELECT user_id, expires_at, attempts, email_hash, email_expires, email_sent_at, email_sends
                 FROM mfa_tickets WHERE token_hash = ?1 AND expires_at > ?2",
                params![token_hash, now()],
                |r| {
                    Ok(MfaTicket {
                        user_id: r.get(0)?,
                        expires_at: r.get(1)?,
                        attempts: r.get(2)?,
                        email_hash: r.get(3)?,
                        email_expires: r.get(4)?,
                        email_sent_at: r.get(5)?,
                        email_sends: r.get(6)?,
                    })
                },
            )
            .optional()
        })
        .await
    }

    /// Counts a wrong code against the challenge and returns the new total.
    pub async fn mfa_ticket_fail(&self, token_hash: String) -> rusqlite::Result<i64> {
        self.run(move |c| {
            c.execute(
                "UPDATE mfa_tickets SET attempts = attempts + 1 WHERE token_hash = ?1",
                [&token_hash],
            )?;
            c.query_row(
                "SELECT attempts FROM mfa_tickets WHERE token_hash = ?1",
                [&token_hash],
                |r| r.get(0),
            )
            .optional()
            .map(|a| a.unwrap_or(i64::MAX))
        })
        .await
    }

    /// Records an emailed code (as a hash) on the challenge.
    pub async fn mfa_ticket_email(
        &self,
        token_hash: String,
        code_hash: String,
        expires_at: i64,
    ) -> rusqlite::Result<()> {
        self.run(move |c| {
            c.execute(
                "UPDATE mfa_tickets SET email_hash = ?2, email_expires = ?3, email_sent_at = ?4,
                 email_sends = email_sends + 1 WHERE token_hash = ?1",
                params![token_hash, code_hash, expires_at, now()],
            )
            .map(|_| ())
        })
        .await
    }

    pub async fn mfa_ticket_delete(&self, token_hash: String) -> rusqlite::Result<()> {
        self.run(move |c| {
            c.execute(
                "DELETE FROM mfa_tickets WHERE token_hash = ?1",
                [token_hash],
            )
            .map(|_| ())
        })
        .await
    }

    /// The authenticator secret and the last time step accepted from it.
    pub async fn totp_state(&self, user_id: i64) -> rusqlite::Result<Option<(String, i64)>> {
        self.run(move |c| {
            c.query_row(
                "SELECT totp_secret, totp_last_step FROM users WHERE id = ?1 AND totp_secret IS NOT NULL",
                [user_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
        })
        .await
    }

    /// Accepts a time step once: false if that step, or a later one, was used already.
    pub async fn totp_use_step(&self, user_id: i64, step: i64) -> rusqlite::Result<bool> {
        self.run(move |c| {
            c.execute(
                "UPDATE users SET totp_last_step = ?2 WHERE id = ?1 AND totp_last_step < ?2",
                params![user_id, step],
            )
            .map(|n| n == 1)
        })
        .await
    }

    pub async fn totp_enable(
        &self,
        user_id: i64,
        secret: String,
        step: i64,
    ) -> rusqlite::Result<()> {
        self.run(move |c| {
            let tx = c.transaction()?;
            tx.execute(
                "UPDATE users SET totp_secret = ?2, totp_last_step = ?3 WHERE id = ?1",
                params![user_id, secret, step],
            )?;
            tx.execute("DELETE FROM mfa_setup WHERE user_id = ?1", [user_id])?;
            tx.commit()
        })
        .await
    }

    pub async fn totp_disable(&self, user_id: i64) -> rusqlite::Result<()> {
        self.run(move |c| {
            c.execute(
                "UPDATE users SET totp_secret = NULL, totp_last_step = 0 WHERE id = ?1",
                [user_id],
            )
            .map(|_| ())
        })
        .await
    }

    pub async fn mfa_email_set(&self, user_id: i64, on: bool) -> rusqlite::Result<()> {
        self.run(move |c| {
            c.execute(
                "UPDATE users SET mfa_email = ?2 WHERE id = ?1",
                params![user_id, on],
            )
            .map(|_| ())
        })
        .await
    }

    pub async fn mfa_setup_put(
        &self,
        user_id: i64,
        secret: String,
        expires_at: i64,
    ) -> rusqlite::Result<()> {
        self.run(move |c| {
            c.execute(
                "INSERT INTO mfa_setup (user_id, secret, expires_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(user_id) DO UPDATE SET secret = ?2, expires_at = ?3",
                params![user_id, secret, expires_at],
            )
            .map(|_| ())
        })
        .await
    }

    pub async fn mfa_setup_get(&self, user_id: i64) -> rusqlite::Result<Option<String>> {
        self.run(move |c| {
            c.query_row(
                "SELECT secret FROM mfa_setup WHERE user_id = ?1 AND expires_at > ?2",
                params![user_id, now()],
                |r| r.get(0),
            )
            .optional()
        })
        .await
    }

    /// Replaces the user's recovery codes with these (hashes).
    pub async fn recovery_replace(
        &self,
        user_id: i64,
        hashes: Vec<String>,
    ) -> rusqlite::Result<()> {
        self.run(move |c| {
            let tx = c.transaction()?;
            tx.execute("DELETE FROM recovery_codes WHERE user_id = ?1", [user_id])?;
            for h in hashes {
                tx.execute(
                    "INSERT OR IGNORE INTO recovery_codes (user_id, code_hash) VALUES (?1, ?2)",
                    params![user_id, h],
                )?;
            }
            tx.commit()
        })
        .await
    }

    /// Uses a recovery code: true if it was there, and now it is not.
    pub async fn recovery_use(&self, user_id: i64, hash: String) -> rusqlite::Result<bool> {
        self.run(move |c| {
            c.execute(
                "DELETE FROM recovery_codes WHERE user_id = ?1 AND code_hash = ?2",
                params![user_id, hash],
            )
            .map(|n| n == 1)
        })
        .await
    }

    pub async fn recovery_count(&self, user_id: i64) -> rusqlite::Result<i64> {
        self.run(move |c| {
            c.query_row(
                "SELECT COUNT(*) FROM recovery_codes WHERE user_id = ?1",
                [user_id],
                |r| r.get(0),
            )
        })
        .await
    }

    /// Removes every second step from an account: the secret, email codes, recovery
    /// codes, an enrollment in progress and any waiting login challenge.
    pub async fn mfa_clear(&self, user_id: i64) -> rusqlite::Result<()> {
        self.run(move |c| {
            let tx = c.transaction()?;
            tx.execute(
                "UPDATE users SET totp_secret = NULL, totp_last_step = 0, mfa_email = 0 WHERE id = ?1",
                [user_id],
            )?;
            tx.execute("DELETE FROM recovery_codes WHERE user_id = ?1", [user_id])?;
            tx.execute("DELETE FROM mfa_setup WHERE user_id = ?1", [user_id])?;
            tx.execute("DELETE FROM mfa_tickets WHERE user_id = ?1", [user_id])?;
            tx.commit()
        })
        .await
    }

    // ---- live instances ----------------------------------------------------

    pub async fn live_create(&self, i: Instance) -> rusqlite::Result<()> {
        self.run(move |c| {
            c.execute(
                "INSERT INTO live_instances (label, user_id, pack_id, seed, created_at, expires_at, keyed)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    i.label,
                    i.user_id,
                    i.pack_id,
                    i.seed,
                    i.created_at,
                    i.expires_at,
                    i.keyed
                ],
            )
            .map(|_| ())
        })
        .await
    }

    /// The instance behind a subdomain, live or expired.
    pub async fn live_by_label(&self, label: String) -> rusqlite::Result<Option<Instance>> {
        self.run(move |c| {
            c.query_row(
                &format!("SELECT {INSTANCE_COLS} FROM live_instances WHERE label = ?1"),
                [label],
                row_instance,
            )
            .optional()
        })
        .await
    }

    /// The user's unexpired instance of a pack, if any.
    pub async fn live_active(
        &self,
        user_id: i64,
        pack_id: String,
    ) -> rusqlite::Result<Option<Instance>> {
        self.run(move |c| {
            c.query_row(
                &format!(
                    "SELECT {INSTANCE_COLS} FROM live_instances
                     WHERE user_id = ?1 AND pack_id = ?2 AND expires_at > ?3
                     ORDER BY created_at DESC LIMIT 1"
                ),
                params![user_id, pack_id, now()],
                row_instance,
            )
            .optional()
        })
        .await
    }

    /// How many unexpired instances the user has, across all packs.
    pub async fn live_active_count(&self, user_id: i64) -> rusqlite::Result<i64> {
        self.run(move |c| {
            c.query_row(
                "SELECT COUNT(*) FROM live_instances WHERE user_id = ?1 AND expires_at > ?2",
                params![user_id, now()],
                |r| r.get(0),
            )
        })
        .await
    }

    /// Every instance the user has had of a pack, newest first, expired ones
    /// included while they are kept. A submitted flag is checked against these.
    pub async fn live_of(&self, user_id: i64, pack_id: String) -> rusqlite::Result<Vec<Instance>> {
        self.run(move |c| {
            let mut st = c.prepare(&format!(
                "SELECT {INSTANCE_COLS} FROM live_instances
                 WHERE user_id = ?1 AND pack_id = ?2 ORDER BY created_at DESC LIMIT 20"
            ))?;
            let rows = st.query_map(params![user_id, pack_id], row_instance)?;
            rows.collect()
        })
        .await
    }

    /// Labels of every unexpired instance.
    pub async fn live_active_labels(&self) -> rusqlite::Result<std::collections::HashSet<String>> {
        self.run(|c| {
            let mut st = c.prepare("SELECT label FROM live_instances WHERE expires_at > ?1")?;
            let rows = st.query_map([now()], |r| r.get::<_, String>(0))?;
            rows.collect()
        })
        .await
    }

    pub async fn live_set_expiry(&self, label: String, expires_at: i64) -> rusqlite::Result<()> {
        self.run(move |c| {
            c.execute(
                "UPDATE live_instances SET expires_at = ?2 WHERE label = ?1",
                params![label, expires_at],
            )
            .map(|_| ())
        })
        .await
    }

    pub async fn live_delete(&self, label: String) -> rusqlite::Result<()> {
        self.run(move |c| {
            c.execute("DELETE FROM live_instances WHERE label = ?1", [label])
                .map(|_| ())
        })
        .await
    }

    pub async fn live_set_flags(&self, label: String, flags: Vec<String>) -> rusqlite::Result<()> {
        self.run(move |c| {
            c.execute(
                "UPDATE live_instances SET flags = ?2 WHERE label = ?1",
                params![label, flags.join(",")],
            )
            .map(|_| ())
        })
        .await
    }

    /// Ends the user's running instances of a pack now.
    pub async fn live_stop(&self, user_id: i64, pack_id: String) -> rusqlite::Result<()> {
        self.run(move |c| {
            c.execute(
                "UPDATE live_instances SET expires_at = ?3
                 WHERE user_id = ?1 AND pack_id = ?2 AND expires_at > ?3",
                params![user_id, pack_id, now()],
            )
            .map(|_| ())
        })
        .await
    }

    // ---- housekeeping ------------------------------------------------------

    /// Drops expired sessions and tokens, and accounts that never verified.
    pub async fn purge_expired(&self) -> rusqlite::Result<()> {
        self.run(|c| {
            let t = now();
            c.execute("DELETE FROM sessions WHERE expires_at <= ?1", [t])?;
            c.execute("DELETE FROM email_tokens WHERE expires_at <= ?1", [t])?;
            c.execute("DELETE FROM mfa_tickets WHERE expires_at <= ?1", [t])?;
            c.execute("DELETE FROM mfa_setup WHERE expires_at <= ?1", [t])?;
            c.execute(
                "DELETE FROM progress WHERE started_at <= ?1",
                [t - 30 * 24 * 3600],
            )?;
            c.execute(
                "DELETE FROM live_instances WHERE expires_at <= ?1",
                [t - LIVE_KEEP_SECS],
            )?;
            c.execute(
                "DELETE FROM users WHERE verified = 0 AND is_admin = 0 AND created_at <= ?1",
                [t - UNVERIFIED_TTL_SECS],
            )?;
            Ok(())
        })
        .await
    }
}

fn classify_unique(e: rusqlite::Error) -> CreateError {
    if let rusqlite::Error::SqliteFailure(f, Some(msg)) = &e {
        if f.code == ErrorCode::ConstraintViolation {
            if msg.contains("users.username") {
                return CreateError::UsernameTaken;
            }
            if msg.contains("users.email") {
                return CreateError::EmailTaken;
            }
        }
    }
    CreateError::Other(e.to_string())
}
