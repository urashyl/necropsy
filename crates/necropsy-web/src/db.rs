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

const SCHEMA_VERSION: i64 = 1;

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
    PRIMARY KEY (user_id, pack_id)
);
";

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
}

#[derive(Debug)]
pub struct Solve {
    pub pack_id: String,
    pub solved_at: i64,
    pub seconds: i64,
    pub hints: i64,
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
    })
}

const USER_COLS: &str = "id, username, is_admin, verified, disabled, created_at";

impl Db {
    /// Opens (creating if needed) the database at `path`; `:memory:` works.
    pub fn open(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version == 0 {
            conn.execute_batch(SCHEMA)?;
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        } else if version != SCHEMA_VERSION {
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "database schema version {version} is not supported (expected {SCHEMA_VERSION})"
            )));
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
                |r| Ok((row_user(r)?, r.get(6)?)),
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
                        (SELECT COUNT(*) FROM solves s WHERE s.user_id = u.id)
                 FROM users u ORDER BY u.created_at DESC, u.id DESC",
            )?;
            let rows = st.query_map([], |r| {
                Ok(AdminRow {
                    user: row_user(r)?,
                    solves: r.get(6)?,
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
                "SELECT u.id, u.username, u.is_admin, u.verified, u.disabled, u.created_at
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

    /// Records a first solve. Later solves of the same pack change nothing.
    pub async fn record_solve(
        &self,
        user_id: i64,
        pack_id: String,
        seconds: i64,
        hints: i64,
    ) -> rusqlite::Result<()> {
        self.run(move |c| {
            c.execute(
                "INSERT INTO solves (user_id, pack_id, solved_at, seconds, hints)
                 VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT DO NOTHING",
                params![user_id, pack_id, now(), seconds, hints],
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
                "SELECT pack_id, solved_at, seconds, hints FROM solves WHERE user_id = ?1 ORDER BY solved_at",
            )?;
            let rows = st.query_map([user_id], |r| {
                Ok(Solve {
                    pack_id: r.get(0)?,
                    solved_at: r.get(1)?,
                    seconds: r.get(2)?,
                    hints: r.get(3)?,
                })
            })?;
            rows.collect()
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
