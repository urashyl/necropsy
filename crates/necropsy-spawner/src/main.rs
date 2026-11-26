// SPDX-License-Identifier: AGPL-3.0-or-later
//! The spawner: runs the sandboxed lab containers of live puzzles, for the site
//! servers, on a VM of their own. See `docs/range.md`.
//!
//! It is the only thing that talks to Docker on that VM, and it serves several
//! tenants (one per site, such as production and dev) over HTTP on the private
//! network. Each tenant has a token, its own labs and its own limits:
//!
//! - **The control API** starts, extends and stops labs, takes the images a site
//!   builds, replaces its lab allowlist and runs the self-test.
//! - **Lab traffic** is forwarded: a site passes a player's request here with the
//!   lab's id in `X-Necropsy-Instance`, and it reaches that lab's unix socket.
//!
//! Every lab is started with no network at all. The only way in is a unix socket
//! in a directory on this VM, and the only thing that connects to it is this
//! program. A site can ask for a lab of an image it pushed, with the limits it
//! pushed, within its tenant's caps, and nothing else: no arbitrary image, no
//! mounts, no network, and no way to see another tenant's labs.

mod config;
mod docker;
mod images;
mod proxy;
mod selftest;
mod uds;

use std::collections::{BTreeMap, HashMap};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Extension, Path, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::config::{Config, Image};
use crate::docker::Docker;

/// Longest a lab may be asked to live in one go.
const MAX_TTL_SECS: i64 = 8 * 3600;
/// How long a started lab gets to open its socket.
const READY_TIMEOUT: Duration = Duration::from_secs(20);
/// Environment variables a caller may set. The values are checked too.
const ENV_ALLOWED: &[&str] = &["NECROPSY_FLAG", "NECROPSY_INSTANCE"];
/// Largest allowlist a site may push.
const MAX_ALLOWLIST: usize = 64 * 1024;

const TOKEN_HEADER: &str = "x-necropsy-token";
const INSTANCE_HEADER: &str = "x-necropsy-instance";

pub struct Inst {
    pub image: String,
    pub memory_mb: u64,
    pub expires: i64,
}

type Key = (String, String);
type Allowlist = BTreeMap<String, Image>;

pub struct App {
    pub cfg: Config,
    pub docker: Docker,
    live: Mutex<HashMap<Key, Inst>>,
    allow: RwLock<HashMap<String, Allowlist>>,
    /// One image upload at a time: they are large, and are spooled to disk.
    upload: tokio::sync::Mutex<()>,
}

/// Who is asking, from their token.
#[derive(Clone)]
pub struct TenantName(pub String);

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A lab id: what the site uses as the lab's subdomain.
pub fn valid_id(id: &str) -> bool {
    (1..=40).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

fn valid_env_value(v: &str) -> bool {
    v.len() <= 200
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"{}_-.".contains(&b))
}

pub fn container_name(tenant: &str, id: &str) -> String {
    format!("nr-{tenant}-{id}")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Constant-time equality of two strings of the same length.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

#[derive(Deserialize)]
pub struct CreateReq {
    pub id: String,
    pub image: String,
    pub ttl_secs: i64,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

#[derive(Deserialize)]
struct TouchReq {
    ttl_secs: i64,
}

#[derive(Serialize)]
struct InstView {
    id: String,
    image: String,
    expires: i64,
}

pub type Reply = (StatusCode, String);

fn reply(code: StatusCode, msg: impl Into<String>) -> Reply {
    (code, msg.into())
}

impl App {
    pub fn new(cfg: Config, docker: Docker) -> Arc<Self> {
        Arc::new(Self {
            cfg,
            docker,
            live: Mutex::new(HashMap::new()),
            allow: RwLock::new(HashMap::new()),
            upload: tokio::sync::Mutex::new(()),
        })
    }

    pub fn dir(&self, tenant: &str, id: &str) -> PathBuf {
        self.cfg.range_dir.join(tenant).join(id)
    }

    pub fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<Key, Inst>> {
        self.live.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn allowlist_of(&self, tenant: &str) -> Allowlist {
        self.allow
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(tenant)
            .cloned()
            .unwrap_or_default()
    }

    fn state_file(&self, tenant: &str, name: &str) -> PathBuf {
        self.cfg.state_dir.join(tenant).join(name)
    }

    /// The tenant a token belongs to. Every tenant is compared, so how long it
    /// takes does not say which one matched.
    fn tenant_for(&self, token: &str) -> Option<String> {
        let digest = hex(&Sha256::digest(token.as_bytes()));
        let mut found = None;
        for (name, t) in &self.cfg.tenants {
            if same(&digest, &t.token_sha256) {
                found = Some(name.clone());
            }
        }
        found
    }

    /// Removes the container and its socket directory. Never fails: whatever
    /// is already gone is fine.
    pub async fn teardown(&self, tenant: &str, id: &str) {
        let _ = self
            .docker
            .run(&["rm", "-f", &container_name(tenant, id)])
            .await;
        let _ = tokio::fs::remove_dir_all(self.dir(tenant, id)).await;
        self.lock().remove(&(tenant.to_owned(), id.to_owned()));
    }

    pub async fn create(&self, tenant: &str, req: CreateReq) -> Result<(), Reply> {
        if !valid_id(&req.id) {
            return Err(reply(StatusCode::BAD_REQUEST, "bad id"));
        }
        let Some(img) = self.allowlist_of(tenant).get(&req.image).cloned() else {
            return Err(reply(
                StatusCode::BAD_REQUEST,
                "that lab is not on the allowlist",
            ));
        };
        if !(1..=MAX_TTL_SECS).contains(&req.ttl_secs) {
            return Err(reply(StatusCode::BAD_REQUEST, "bad ttl"));
        }
        for (k, v) in &req.env {
            if !ENV_ALLOWED.contains(&k.as_str()) || !valid_env_value(v) {
                return Err(reply(StatusCode::BAD_REQUEST, "bad environment"));
            }
        }
        let limits = &self.cfg.tenants[tenant];
        {
            let mut live = self.lock();
            let key = (tenant.to_owned(), req.id.clone());
            if live.contains_key(&key) {
                return Err(reply(StatusCode::CONFLICT, "exists"));
            }
            let (count, memory) = live
                .iter()
                .filter(|((t, _), _)| t == tenant)
                .fold((0usize, 0u64), |(c, m), (_, i)| (c + 1, m + i.memory_mb));
            if count >= limits.max_instances {
                return Err(reply(StatusCode::TOO_MANY_REQUESTS, "the range is full"));
            }
            if memory + u64::from(img.memory_mb) > limits.memory_budget_mb {
                return Err(reply(StatusCode::TOO_MANY_REQUESTS, "the range is full"));
            }
            // Held from here on, so concurrent requests count it.
            live.insert(
                key,
                Inst {
                    image: req.image.clone(),
                    memory_mb: u64::from(img.memory_mb),
                    expires: now() + req.ttl_secs,
                },
            );
        }
        match self.start(tenant, &req, &img).await {
            Ok(()) => Ok(()),
            Err(e) => {
                tracing::error!("starting {tenant}/{} failed: {e}", req.id);
                self.teardown(tenant, &req.id).await;
                Err(reply(StatusCode::BAD_GATEWAY, "could not start the lab"))
            }
        }
    }

    async fn start(&self, tenant: &str, req: &CreateReq, img: &Image) -> Result<(), String> {
        let dir = self.dir(tenant, &req.id);
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| e.to_string())?;
        // The lab's user owns the directory, to create its socket there. Only
        // this program connects to it.
        std::os::unix::fs::chown(&dir, Some(img.uid), Some(img.uid))
            .map_err(|e| format!("chown: {e}"))?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;

        // The flag goes to the lab as a file, never in its environment: with a process
        // supervisor at PID 1 an environment variable is readable from /proc/1/environ
        // by anyone with a shell in the lab, which would skip every puzzle. The lab
        // reads the file once, as it starts, and deletes it (see docs/range.md).
        let mut env = req.env.clone();
        if let Some(flag) = env.remove("NECROPSY_FLAG") {
            let path = dir.join("flag");
            tokio::fs::write(&path, flag)
                .await
                .map_err(|e| e.to_string())?;
            std::os::unix::fs::chown(&path, Some(img.uid), Some(img.uid))
                .map_err(|e| format!("chown: {e}"))?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400))
                .map_err(|e| e.to_string())?;
        }

        let expires = self
            .lock()
            .get(&(tenant.to_owned(), req.id.clone()))
            .map(|i| i.expires)
            .unwrap_or(0);
        let args = docker::run_args(
            &container_name(tenant, &req.id),
            tenant,
            &req.image,
            img,
            &dir,
            expires,
            &env,
        );
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        self.docker.run(&refs).await?;

        // Ready when the lab has opened its socket.
        let sock = dir.join("app.sock");
        let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
        while !sock.exists() {
            if tokio::time::Instant::now() > deadline {
                return Err("the lab never opened its socket".into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Ok(())
    }

    /// Once at startup: load each tenant's allowlist, forget no lab that is
    /// still running, remove those that are expired, and remove socket
    /// directories nothing owns.
    async fn recover(&self) {
        for tenant in self.cfg.tenants.keys() {
            let path = self.state_file(tenant, "images.toml");
            if let Ok(text) = tokio::fs::read_to_string(&path).await {
                match config::parse_images(&text, tenant) {
                    Ok(list) => {
                        self.allow
                            .write()
                            .unwrap_or_else(|p| p.into_inner())
                            .insert(tenant.clone(), list);
                    }
                    Err(e) => tracing::error!("{}: {e}", path.display()),
                }
            }
        }
        let now = now();
        match self.docker.list().await {
            Ok(rows) => {
                for l in rows {
                    if !self.cfg.tenants.contains_key(&l.tenant) {
                        continue;
                    }
                    if l.expires <= now {
                        self.teardown(&l.tenant, &l.id).await;
                    } else {
                        self.lock().insert(
                            (l.tenant, l.id),
                            Inst {
                                image: l.image,
                                memory_mb: l.memory_mb,
                                expires: l.expires,
                            },
                        );
                    }
                }
            }
            Err(e) => tracing::error!("cannot list containers: {e}"),
        }
        for tenant in self.cfg.tenants.keys() {
            let Ok(mut rd) = tokio::fs::read_dir(self.cfg.range_dir.join(tenant)).await else {
                continue;
            };
            while let Ok(Some(e)) = rd.next_entry().await {
                let id = e.file_name().to_string_lossy().into_owned();
                if !self.lock().contains_key(&(tenant.clone(), id)) {
                    let _ = tokio::fs::remove_dir_all(e.path()).await;
                }
            }
        }
    }

    async fn reap(&self) {
        let now = now();
        let expired: Vec<Key> = self
            .lock()
            .iter()
            .filter(|(_, i)| i.expires <= now)
            .map(|(k, _)| k.clone())
            .collect();
        for (tenant, id) in expired {
            tracing::info!("reaping {tenant}/{id}");
            self.teardown(&tenant, &id).await;
        }
    }

    /// Removes a tenant's images that neither its current nor its previous
    /// allowlist names. Docker refuses to remove one a running lab uses, which
    /// is what keeps those.
    async fn prune_images(&self, tenant: &str) {
        let mut keep: Vec<String> = Vec::new();
        for name in ["images.toml", "images.prev.toml"] {
            if let Ok(text) = tokio::fs::read_to_string(self.state_file(tenant, name)).await {
                if let Ok(list) = config::parse_images(&text, tenant) {
                    keep.extend(list.into_values().map(|i| i.image));
                }
            }
        }
        let reference = format!("reference=necropsy-range/{tenant}/*");
        let Ok(out) = self
            .docker
            .run(&[
                "image",
                "ls",
                "--filter",
                &reference,
                "--format",
                "{{.Repository}}:{{.Tag}}",
            ])
            .await
        else {
            return;
        };
        for image in out.lines().map(str::trim).filter(|l| !l.is_empty()) {
            if !keep.iter().any(|k| k == image) {
                let _ = self.docker.run(&["rmi", image]).await;
            }
        }
    }
}

// ----------------------------------------------------------------------------- auth

/// Every request but the health check carries a tenant's token. A request with
/// `X-Necropsy-Instance` is lab traffic and is forwarded; the rest is the control
/// API.
async fn auth(State(app): State<Arc<App>>, mut req: Request, next: Next) -> Response {
    if req.uri().path() == "/healthz" && !req.headers().contains_key(INSTANCE_HEADER) {
        return next.run(req).await;
    }
    let tenant = req
        .headers()
        .get(TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .filter(|t| (16..=256).contains(&t.len()))
        .and_then(|t| app.tenant_for(t));
    let Some(tenant) = tenant else {
        return (
            StatusCode::UNAUTHORIZED,
            "a valid X-Necropsy-Token is required",
        )
            .into_response();
    };
    if let Some(id) = req
        .headers()
        .get(INSTANCE_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
    {
        return proxy::handle(&app, &tenant, &id, req).await;
    }
    req.extensions_mut().insert(TenantName(tenant));
    next.run(req).await
}

// ----------------------------------------------------------------------------- handlers

async fn create(
    State(app): State<Arc<App>>,
    Extension(TenantName(t)): Extension<TenantName>,
    Json(req): Json<CreateReq>,
) -> Reply {
    match app.create(&t, req).await {
        Ok(()) => reply(StatusCode::CREATED, "created"),
        Err(r) => r,
    }
}

async fn touch(
    State(app): State<Arc<App>>,
    Extension(TenantName(t)): Extension<TenantName>,
    Path(id): Path<String>,
    Json(req): Json<TouchReq>,
) -> Reply {
    if !(1..=MAX_TTL_SECS).contains(&req.ttl_secs) {
        return reply(StatusCode::BAD_REQUEST, "bad ttl");
    }
    match app.lock().get_mut(&(t, id)) {
        Some(i) => {
            i.expires = now() + req.ttl_secs;
            reply(StatusCode::OK, "ok")
        }
        None => reply(StatusCode::NOT_FOUND, "no such lab"),
    }
}

async fn stop(
    State(app): State<Arc<App>>,
    Extension(TenantName(t)): Extension<TenantName>,
    Path(id): Path<String>,
) -> Reply {
    if !valid_id(&id) {
        return reply(StatusCode::BAD_REQUEST, "bad id");
    }
    app.teardown(&t, &id).await;
    reply(StatusCode::OK, "stopped")
}

async fn list(
    State(app): State<Arc<App>>,
    Extension(TenantName(t)): Extension<TenantName>,
) -> Json<Vec<InstView>> {
    let mut v: Vec<InstView> = app
        .lock()
        .iter()
        .filter(|((tenant, _), _)| *tenant == t)
        .map(|((_, id), i)| InstView {
            id: id.clone(),
            image: i.image.clone(),
            expires: i.expires,
        })
        .collect();
    v.sort_by(|a, b| a.id.cmp(&b.id));
    Json(v)
}

async fn get_allowlist(
    State(app): State<Arc<App>>,
    Extension(TenantName(t)): Extension<TenantName>,
) -> Reply {
    match tokio::fs::read_to_string(app.state_file(&t, "images.toml")).await {
        Ok(text) => reply(StatusCode::OK, text),
        Err(_) => reply(StatusCode::OK, ""),
    }
}

/// Replaces the tenant's allowlist. Every lab it names must be under the
/// tenant's own image path and already loaded here, so a bad push changes
/// nothing.
async fn put_allowlist(
    State(app): State<Arc<App>>,
    Extension(TenantName(t)): Extension<TenantName>,
    text: String,
) -> Reply {
    if text.len() > MAX_ALLOWLIST {
        return reply(StatusCode::PAYLOAD_TOO_LARGE, "the allowlist is too large");
    }
    let list = match config::parse_images(&text, &t) {
        Ok(l) => l,
        Err(e) => return reply(StatusCode::BAD_REQUEST, e),
    };
    for (name, img) in &list {
        if !app.docker.image_exists(&img.image).await {
            return reply(
                StatusCode::BAD_REQUEST,
                format!(
                    "lab {name:?}: the image {} is not on this server, push it first",
                    img.image
                ),
            );
        }
    }
    let dir = app.cfg.state_dir.join(&t);
    let current = app.state_file(&t, "images.toml");
    let write = async {
        tokio::fs::create_dir_all(&dir).await?;
        if tokio::fs::try_exists(&current).await.unwrap_or(false) {
            tokio::fs::copy(&current, app.state_file(&t, "images.prev.toml")).await?;
        }
        let tmp = app.state_file(&t, "images.toml.new");
        tokio::fs::write(&tmp, &text).await?;
        tokio::fs::rename(&tmp, &current).await
    };
    if let Err(e) = write.await {
        tracing::error!("saving {t}'s allowlist failed: {e}");
        return reply(
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not save the allowlist",
        );
    }
    let n = list.len();
    app.allow
        .write()
        .unwrap_or_else(|p| p.into_inner())
        .insert(t.clone(), list);
    app.prune_images(&t).await;
    reply(StatusCode::OK, format!("{n} lab(s) on the allowlist"))
}

/// The tenant's lab images this server has, one per line, so a site pushes only
/// what is missing.
async fn list_images(
    State(app): State<Arc<App>>,
    Extension(TenantName(t)): Extension<TenantName>,
) -> Reply {
    let reference = format!("reference=necropsy-range/{t}/*");
    match app
        .docker
        .run(&[
            "image",
            "ls",
            "--filter",
            &reference,
            "--format",
            "{{.Repository}}:{{.Tag}}",
        ])
        .await
    {
        Ok(out) => reply(StatusCode::OK, out),
        Err(e) => {
            tracing::error!("listing images failed: {e}");
            reply(StatusCode::BAD_GATEWAY, "docker could not list images")
        }
    }
}

/// Takes a lab image the site built, as a `docker save` archive.
async fn upload_image(
    State(app): State<Arc<App>>,
    Extension(TenantName(t)): Extension<TenantName>,
    body: Body,
) -> Reply {
    let Ok(_one_at_a_time) = app.upload.try_lock() else {
        return reply(
            StatusCode::TOO_MANY_REQUESTS,
            "another image is being uploaded",
        );
    };
    let uploads = app.cfg.state_dir.join("uploads");
    if tokio::fs::create_dir_all(&uploads).await.is_err() {
        return reply(StatusCode::INTERNAL_SERVER_ERROR, "no room for uploads");
    }
    let path = uploads.join(format!("{t}-{}.tar", selftest_id()));
    let result = async {
        let mut file = tokio::fs::File::create(&path)
            .await
            .map_err(|_| reply(StatusCode::INTERNAL_SERVER_ERROR, "no room for uploads"))?;
        let mut size = 0u64;
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            let chunk =
                chunk.map_err(|_| reply(StatusCode::BAD_REQUEST, "the upload broke off"))?;
            size += chunk.len() as u64;
            if size > images::MAX_BYTES {
                return Err(reply(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "the image is too large",
                ));
            }
            file.write_all(&chunk)
                .await
                .map_err(|_| reply(StatusCode::INTERNAL_SERVER_ERROR, "no room for uploads"))?;
        }
        file.flush().await.ok();
        drop(file);

        let (check_path, tenant) = (path.clone(), t.clone());
        let tags = tokio::task::spawn_blocking(move || images::check_archive(&check_path, &tenant))
            .await
            .map_err(|_| reply(StatusCode::INTERNAL_SERVER_ERROR, "check failed"))?
            .map_err(|e| reply(StatusCode::BAD_REQUEST, e))?;
        app.docker.load(&path).await.map_err(|e| {
            tracing::error!("docker load failed: {e}");
            reply(StatusCode::BAD_GATEWAY, "docker could not load the image")
        })?;
        Ok::<_, Reply>(tags)
    }
    .await;
    let _ = tokio::fs::remove_file(&path).await;
    match result {
        Ok(tags) => reply(StatusCode::OK, tags.join("\n")),
        Err(r) => r,
    }
}

fn selftest_id() -> String {
    use std::io::Read;
    let mut b = [0u8; 6];
    let _ = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b));
    hex(&b)
}

/// Starts each of the tenant's labs and checks it, see `selftest`.
async fn run_selftest(
    State(app): State<Arc<App>>,
    Extension(TenantName(t)): Extension<TenantName>,
) -> Reply {
    let labs: Vec<String> = app.allowlist_of(&t).into_keys().collect();
    if labs.is_empty() {
        return reply(
            StatusCode::BAD_REQUEST,
            "the tenant has no labs on its allowlist",
        );
    }
    let mut report = String::new();
    let mut failed = false;
    for lab in &labs {
        match selftest::check_lab(&app, &t, lab).await {
            Ok(lines) => {
                for l in lines {
                    report.push_str("ok: ");
                    report.push_str(&l);
                    report.push('\n');
                }
            }
            Err(e) => {
                failed = true;
                report.push_str("FAIL: ");
                report.push_str(&e);
                report.push('\n');
            }
        }
    }
    if failed {
        reply(StatusCode::INTERNAL_SERVER_ERROR, report)
    } else {
        report.push_str(&format!("all {} lab(s) passed\n", labs.len()));
        reply(StatusCode::OK, report)
    }
}

pub fn router(app: Arc<App>) -> Router {
    let json_routes = Router::new()
        .route("/v1/instances", post(create).get(list))
        .route("/v1/instances/{id}", delete(stop))
        .route("/v1/instances/{id}/touch", post(touch))
        .route("/v1/allowlist", get(get_allowlist).put(put_allowlist))
        .route("/v1/selftest", post(run_selftest))
        .layer(DefaultBodyLimit::max(MAX_ALLOWLIST));
    Router::new()
        .merge(json_routes)
        // Images are large and are streamed to disk, with their own cap.
        .route(
            "/v1/images",
            get(list_images)
                .post(upload_image)
                .layer(DefaultBodyLimit::disable()),
        )
        .route("/healthz", get(|| async { "ok" }))
        .layer(middleware::from_fn_with_state(app.clone(), auth))
        .with_state(app)
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "necropsy_spawner=info".into()),
        )
        .init();
    let cfg = Config::from_env().unwrap_or_else(|e| {
        eprintln!("error: {e}");
        std::process::exit(1)
    });
    for dir in [&cfg.state_dir, &cfg.range_dir] {
        std::fs::create_dir_all(dir).expect("cannot create a working directory");
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755))
            .expect("cannot set a directory's mode");
    }
    let listen = cfg.listen;
    let app = App::new(cfg.clone(), Docker::new(cfg.docker.clone()));
    app.recover().await;
    tracing::info!(
        "{} tenants: {}",
        cfg.tenants.len(),
        cfg.tenants.keys().cloned().collect::<Vec<_>>().join(", ")
    );
    {
        let app = app.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(10));
            loop {
                tick.tick().await;
                app.reap().await;
            }
        });
    }

    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .unwrap_or_else(|e| panic!("cannot listen on {listen}: {e}"));
    tracing::info!("listening on http://{listen}");
    axum::serve(listener, router(app))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .expect("server error");
}

#[cfg(test)]
mod tests;
