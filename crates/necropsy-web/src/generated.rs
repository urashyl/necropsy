// SPDX-License-Identifier: AGPL-3.0-or-later
//! Generated packs: each player gets their own files and answer from the pack's
//! generator. See `docs/generators.md`.
//!
//! A player's **seed** is random: one per account and pack, stored the first
//! time they open the puzzle, or, signed out, one per browser from the
//! `necropsy_anon` cookie. The generator never sees it: it gets
//! `HMAC(server secret, pack, seed)` ([`necropsy_pack::gen::seed_for`]), so the
//! database alone does not give anyone's answer.
//!
//! The generator runs in a child process (`necropsy-web generate`) with an
//! empty environment, limits of its own and a deadline; it reads the module and
//! seed on stdin and writes the output on stdout. The server secret, the
//! database and the sessions stay in this process. Outputs are cached in memory.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::HeaderMap;
use necropsy_pack::gen::{self, Output};
use necropsy_pack::Pack;

use crate::db::User;
use crate::state::{AppState, RenderedFile};

/// The cookie that holds a signed-out browser's id.
pub const ANON_COOKIE: &str = "necropsy_anon";
/// Outputs kept in memory.
const CACHE_ENTRIES: usize = 512;
/// A generator that has not answered by then is killed.
const DEADLINE: Duration = Duration::from_secs(10);

/// Where generators run.
pub enum Runner {
    /// A child process: this binary with the `generate` argument.
    Process(PathBuf),
    /// This process. For tests, where there is no server binary to start.
    InProcess,
}

/// One player's output, with its files rendered for the page once.
pub struct Generated {
    pub output: Output,
    pub rendered: Vec<RenderedFile>,
}

/// Outputs by pack and seed, and the order they were added, to drop the oldest.
type Cache = (
    HashMap<(String, String), Arc<Generated>>,
    VecDeque<(String, String)>,
);

pub struct Generator {
    secret: Vec<u8>,
    runner: Runner,
    cache: Mutex<Cache>,
}

impl Default for Generator {
    /// In-process, with a fixed secret: for tests.
    fn default() -> Self {
        Self::new(b"necropsy test secret".to_vec(), Runner::InProcess)
    }
}

impl Generator {
    pub fn new(secret: Vec<u8>, runner: Runner) -> Self {
        Self {
            secret,
            runner,
            cache: Mutex::default(),
        }
    }

    /// The site's secret, also used to key live flags.
    pub fn secret(&self) -> &[u8] {
        &self.secret
    }

    /// The output for one player's seed, from the cache or the generator.
    pub async fn output(
        &self,
        state: &AppState,
        pack: &Pack,
        seed: &str,
    ) -> Result<Arc<Generated>, String> {
        let key = (pack.meta.id.clone(), seed.to_owned());
        if let Some(g) = self
            .cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .0
            .get(&key)
        {
            return Ok(g.clone());
        }
        let wasm = pack.generator.clone().ok_or("not a generated pack")?;
        let gen_seed = gen::seed_for(&self.secret, &pack.meta.id, seed);
        let output = match &self.runner {
            Runner::InProcess => tokio::task::spawn_blocking(move || gen::run(&wasm, gen_seed))
                .await
                .map_err(|e| e.to_string())??,
            Runner::Process(exe) => run_child(exe, &wasm, gen_seed).await?,
        };
        let rendered = state.render_files(&output.files);
        let g = Arc::new(Generated { output, rendered });
        let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        if cache.0.len() >= CACHE_ENTRIES {
            if let Some(old) = cache.1.pop_front() {
                cache.0.remove(&old);
            }
        }
        cache.0.insert(key.clone(), g.clone());
        cache.1.push_back(key);
        Ok(g)
    }
}

/// Runs a generator in a child process: this binary, `generate`, with nothing
/// in its environment. stdin is the seed (32 bytes) and the module; stdout is
/// the output, decoded and checked again here.
async fn run_child(exe: &Path, wasm: &[u8], seed: [u64; 4]) -> Result<Output, String> {
    use tokio::io::AsyncWriteExt;
    let mut child = tokio::process::Command::new(exe)
        .arg("generate")
        .env_clear()
        .current_dir("/")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("cannot start the generator process: {e}"))?;
    let mut input = Vec::with_capacity(32 + wasm.len());
    for w in seed {
        input.extend_from_slice(&w.to_le_bytes());
    }
    input.extend_from_slice(wasm);
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let write = async move {
        stdin.write_all(&input).await?;
        stdin.shutdown().await
    };
    let run = async {
        write
            .await
            .map_err(|e| format!("writing to the generator failed: {e}"))?;
        child
            .wait_with_output()
            .await
            .map_err(|e| format!("the generator process failed: {e}"))
    };
    let out = tokio::time::timeout(DEADLINE, run)
        .await
        .map_err(|_| "the generator did not finish in time".to_owned())??;
    if !out.status.success() {
        let msg: String = String::from_utf8_lossy(&out.stderr)
            .chars()
            .take(500)
            .collect();
        return Err(format!("the generator failed: {}", msg.trim()));
    }
    gen::decode(&out.stdout)
}

/// [`run_child`], blocking: for checking sealed packs' generators at startup,
/// before the server runs. The child's own CPU limit ends a runaway generator.
pub fn run_child_blocking(exe: &Path, wasm: &[u8], seed: [u64; 4]) -> Result<Output, String> {
    use std::io::Write;
    let mut child = std::process::Command::new(exe)
        .arg("generate")
        .env_clear()
        .current_dir("/")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start the generator process: {e}"))?;
    let mut input = Vec::with_capacity(32 + wasm.len());
    for w in seed {
        input.extend_from_slice(&w.to_le_bytes());
    }
    input.extend_from_slice(wasm);
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let out = child
        .wait_with_output()
        .map_err(|e| format!("the generator process failed: {e}"))?;
    let _ = writer.join();
    if !out.status.success() {
        let msg: String = String::from_utf8_lossy(&out.stderr)
            .chars()
            .take(500)
            .collect();
        return Err(format!("the generator failed: {}", msg.trim()));
    }
    gen::decode(&out.stdout)
}

impl Generator {
    /// Runs a generator now, blocking, the way the site runs generators.
    pub fn run_blocking(&self, wasm: &[u8], seed: [u64; 4]) -> Result<Output, String> {
        match &self.runner {
            Runner::InProcess => gen::run(wasm, seed),
            Runner::Process(exe) => run_child_blocking(exe, wasm, seed),
        }
    }
}

/// `necropsy-web generate`: the child's side. Limits its own resources, reads
/// the seed and module from stdin, runs it, and writes the output to stdout.
pub fn child_main() -> std::process::ExitCode {
    use std::io::{Read, Write};
    #[cfg(unix)]
    {
        use rlimit::Resource;
        // CPU seconds, address space, open files, file size, processes.
        let limits = [
            (Resource::CPU, 10),
            (Resource::AS, 1 << 30),
            (Resource::NOFILE, 16),
            (Resource::FSIZE, 0),
            (Resource::CORE, 0),
        ];
        for (r, v) in limits {
            if let Err(e) = r.set(v, v) {
                eprintln!("cannot set a limit: {e}");
                return std::process::ExitCode::FAILURE;
            }
        }
    }
    let mut input = Vec::new();
    if std::io::stdin()
        .take((gen::MAX_MODULE + 32) as u64)
        .read_to_end(&mut input)
        .is_err()
        || input.len() < 32
    {
        eprintln!("expected a seed and a module on stdin");
        return std::process::ExitCode::FAILURE;
    }
    let w = |i: usize| u64::from_le_bytes(input[i * 8..i * 8 + 8].try_into().expect("8 bytes"));
    let seed = [w(0), w(1), w(2), w(3)];
    match gen::run(&input[32..], seed) {
        Ok(o) => {
            let mut out = std::io::stdout().lock();
            if out
                .write_all(&gen::encode(&o))
                .and_then(|_| out.flush())
                .is_err()
            {
                return std::process::ExitCode::FAILURE;
            }
            std::process::ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// Writes a new file only its owner can read (mode 0600), refusing to replace
/// one that exists.
pub fn create_private(path: &Path, text: &str) -> Result<(), String> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(path)
        .map_err(|e| format!("cannot create {}: {e}", path.display()))?;
    use std::io::Write;
    f.write_all(text.as_bytes())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// The server secret from its file, made (random, mode 0600) if it does not exist.
pub fn load_secret(path: &Path) -> Result<Vec<u8>, String> {
    match std::fs::read_to_string(path) {
        Ok(s) => {
            let s = s.trim();
            if s.len() < 32 {
                return Err(format!(
                    "the server secret in {} is shorter than 32 characters",
                    path.display()
                ));
            }
            Ok(s.as_bytes().to_vec())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let secret = crate::auth::new_token();
            create_private(path, &format!("{secret}\n"))?;
            tracing::warn!(
                "created a new server secret in {}: keep it, since every player's generated files and answers depend on it",
                path.display()
            );
            Ok(secret.into_bytes())
        }
        Err(e) => Err(format!(
            "cannot read the server secret {}: {e}",
            path.display()
        )),
    }
}

/// A signed-out browser's id, if its cookie holds one.
pub fn anon_id(headers: &HeaderMap) -> Option<String> {
    crate::auth::cookie_value(headers, ANON_COOKIE)
        .filter(|v| (16..=64).contains(&v.len()) && v.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// The seeds of whoever is asking, the one their files come from first:
/// - signed in: the account's seed for the pack, made the first time; for a
///   browser that already had an id, the seed it used signed out, so the files
///   stay the same after signing in;
/// - signed out: the browser's.
///
/// An answer is right if it is right for any of them, so a puzzle solved signed
/// out still counts when it is uploaded after signing in. Returns the seeds and
/// a `Set-Cookie` to send when the browser needs a new id.
pub async fn seeds_for(
    state: &AppState,
    viewer: Option<&User>,
    headers: &HeaderMap,
    pack: &Pack,
    create: bool,
) -> (Vec<String>, Option<String>) {
    let anon = anon_id(headers);
    let mut set_cookie = None;
    let anon = match anon {
        Some(a) => Some(a),
        None if create && viewer.is_none() => {
            let a = crate::auth::random_string(
                b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
                24,
            );
            set_cookie = Some(format!(
                "{ANON_COOKIE}={a}; Path=/; Max-Age=31536000; HttpOnly; SameSite=Lax{}",
                if state.secure_cookies { "; Secure" } else { "" }
            ));
            Some(a)
        }
        None => None,
    };
    let anon_seed = anon.map(|a| format!("anon:{a}"));
    let mut seeds = Vec::new();
    if let Some(u) = viewer {
        let made = if create {
            let initial = anon_seed.clone().unwrap_or_else(crate::auth::new_token);
            state
                .db
                .gen_seed_get_or_create(u.id, pack.meta.id.clone(), initial)
                .await
                .map(Some)
        } else {
            state.db.gen_seed(u.id, pack.meta.id.clone()).await
        };
        match made {
            Ok(Some(s)) => seeds.push(s),
            Ok(None) => {}
            Err(e) => tracing::error!("loading a generator seed failed: {e}"),
        }
    }
    if let Some(a) = anon_seed {
        if !seeds.contains(&a) {
            seeds.push(a);
        }
    }
    (seeds, set_cookie)
}

/// Whether `guess` is the answer for any of the asker's seeds.
pub async fn answer_matches(
    state: &AppState,
    viewer: Option<&User>,
    headers: &HeaderMap,
    pack: &Pack,
    guess: &str,
) -> bool {
    let (seeds, _) = seeds_for(state, viewer, headers, pack, false).await;
    let want = necropsy_pack::normalize_answer(guess);
    for s in seeds {
        match state.generator.output(state, pack, &s).await {
            Ok(g) if necropsy_pack::normalize_answer(&g.output.answer) == want => return true,
            Ok(_) => {}
            Err(e) => tracing::error!("generator for {} failed: {e}", pack.meta.id),
        }
    }
    false
}
