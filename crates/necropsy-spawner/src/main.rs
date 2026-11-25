// SPDX-License-Identifier: AGPL-3.0-or-later
//! The spawner: the only thing that talks to Docker for container-backed live
//! puzzles. See `docs/range.md`.
//!
//! It runs on the host, as root or a member of the `docker` group, and listens
//! on a unix socket. The web server is not given the Docker socket (that would
//! be root on the host); it can only ask this program for an instance of an
//! image on the allowlist, with the limits set here, and ask it to stop or
//! extend one. Every container is started with no network at all: the only
//! way in is a unix socket in a directory mounted from the host, which the web
//! server proxies to.

mod config;
mod docker;

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::config::{Config, Image};
use crate::docker::Docker;

/// Longest a container may be asked to live in one go.
const MAX_TTL_SECS: i64 = 8 * 3600;
/// How long a started container gets to open its socket.
const READY_TIMEOUT: Duration = Duration::from_secs(20);
/// Environment variables a caller may set. The values are checked too.
const ENV_ALLOWED: &[&str] = &["NECROPSY_FLAG", "NECROPSY_INSTANCE"];

struct Inst {
    image: String,
    expires: i64,
}

pub struct App {
    cfg: Config,
    docker: Docker,
    live: Mutex<HashMap<String, Inst>>,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// An instance id: what the web server uses as the subdomain.
fn valid_id(id: &str) -> bool {
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

fn container_name(id: &str) -> String {
    format!("nr-{id}")
}

#[derive(Deserialize)]
struct CreateReq {
    id: String,
    image: String,
    ttl_secs: i64,
    #[serde(default)]
    env: HashMap<String, String>,
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

type Reply = (StatusCode, &'static str);

impl App {
    pub fn new(cfg: Config, docker: Docker) -> Arc<Self> {
        Arc::new(Self {
            cfg,
            docker,
            live: Mutex::new(HashMap::new()),
        })
    }

    fn dir(&self, id: &str) -> PathBuf {
        self.cfg.range_dir.join(id)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Inst>> {
        self.live.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Removes the container and its socket directory. Never fails: whatever
    /// is already gone is fine.
    async fn teardown(&self, id: &str) {
        let _ = self.docker.run(&["rm", "-f", &container_name(id)]).await;
        let _ = tokio::fs::remove_dir_all(self.dir(id)).await;
        self.lock().remove(id);
    }

    async fn create(&self, req: CreateReq) -> Result<(), Reply> {
        if !valid_id(&req.id) {
            return Err((StatusCode::BAD_REQUEST, "bad id"));
        }
        let Some(img) = self.cfg.images.get(&req.image) else {
            return Err((StatusCode::BAD_REQUEST, "image is not on the allowlist"));
        };
        if !(1..=MAX_TTL_SECS).contains(&req.ttl_secs) {
            return Err((StatusCode::BAD_REQUEST, "bad ttl"));
        }
        for (k, v) in &req.env {
            if !ENV_ALLOWED.contains(&k.as_str()) || !valid_env_value(v) {
                return Err((StatusCode::BAD_REQUEST, "bad environment"));
            }
        }
        {
            let mut live = self.lock();
            if live.contains_key(&req.id) {
                return Err((StatusCode::CONFLICT, "exists"));
            }
            if live.len() >= self.cfg.max_instances {
                return Err((StatusCode::TOO_MANY_REQUESTS, "the range is full"));
            }
            // Held from here on, so concurrent requests count it.
            live.insert(
                req.id.clone(),
                Inst {
                    image: req.image.clone(),
                    expires: now() + req.ttl_secs,
                },
            );
        }
        match self.start(&req, img).await {
            Ok(()) => Ok(()),
            Err(e) => {
                tracing::error!("starting {} failed: {e}", req.id);
                self.teardown(&req.id).await;
                Err((StatusCode::BAD_GATEWAY, "could not start the instance"))
            }
        }
    }

    async fn start(&self, req: &CreateReq, img: &Image) -> Result<(), String> {
        let dir = self.dir(&req.id);
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| e.to_string())?;
        // The container's user owns the directory, to create its socket there.
        // The web server only needs to traverse it and connect.
        std::os::unix::fs::chown(&dir, Some(img.uid), Some(img.uid))
            .map_err(|e| format!("chown: {e}"))?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;

        let expires = self.lock().get(&req.id).map(|i| i.expires).unwrap_or(0);
        let args = docker::run_args(
            &container_name(&req.id),
            &req.image,
            img,
            &dir,
            expires,
            &req.env,
        );
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        self.docker.run(&refs).await?;

        // Ready when the container has opened its socket.
        let sock = dir.join("app.sock");
        let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
        while !sock.exists() {
            if tokio::time::Instant::now() > deadline {
                return Err("the container never opened its socket".into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Ok(())
    }

    /// Once at startup: forget nothing that is still running, remove what
    /// is expired, and remove socket directories nothing owns.
    async fn recover(&self) {
        let now = now();
        match self.docker.list().await {
            Ok(rows) => {
                for (id, image, expires) in rows {
                    if expires <= now {
                        self.teardown(&id).await;
                    } else {
                        self.lock().insert(id, Inst { image, expires });
                    }
                }
            }
            Err(e) => tracing::error!("cannot list containers: {e}"),
        }
        if let Ok(mut rd) = tokio::fs::read_dir(&self.cfg.range_dir).await {
            while let Ok(Some(e)) = rd.next_entry().await {
                let name = e.file_name().to_string_lossy().into_owned();
                if !self.lock().contains_key(&name) {
                    let _ = tokio::fs::remove_dir_all(e.path()).await;
                }
            }
        }
    }

    async fn reap(&self) {
        let now = now();
        let expired: Vec<String> = self
            .lock()
            .iter()
            .filter(|(_, i)| i.expires <= now)
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            tracing::info!("reaping {id}");
            self.teardown(&id).await;
        }
    }
}

async fn create(State(app): State<Arc<App>>, Json(req): Json<CreateReq>) -> Reply {
    match app.create(req).await {
        Ok(()) => (StatusCode::CREATED, "created"),
        Err(r) => r,
    }
}

async fn touch(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Json(req): Json<TouchReq>,
) -> Reply {
    if !(1..=MAX_TTL_SECS).contains(&req.ttl_secs) {
        return (StatusCode::BAD_REQUEST, "bad ttl");
    }
    match app.lock().get_mut(&id) {
        Some(i) => {
            i.expires = now() + req.ttl_secs;
            (StatusCode::OK, "ok")
        }
        None => (StatusCode::NOT_FOUND, "no such instance"),
    }
}

async fn stop(State(app): State<Arc<App>>, Path(id): Path<String>) -> Reply {
    if !valid_id(&id) {
        return (StatusCode::BAD_REQUEST, "bad id");
    }
    app.teardown(&id).await;
    (StatusCode::OK, "stopped")
}

async fn list(State(app): State<Arc<App>>) -> Json<Vec<InstView>> {
    let mut v: Vec<InstView> = app
        .lock()
        .iter()
        .map(|(id, i)| InstView {
            id: id.clone(),
            image: i.image.clone(),
            expires: i.expires,
        })
        .collect();
    v.sort_by(|a, b| a.id.cmp(&b.id));
    Json(v)
}

pub fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/v1/instances", post(create).get(list))
        .route("/v1/instances/{id}", delete(stop))
        .route("/v1/instances/{id}/touch", post(touch))
        .route("/healthz", get(|| async { "ok" }))
        .layer(axum::extract::DefaultBodyLimit::max(8 * 1024))
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
    let socket = cfg.socket.clone();
    let gid = cfg.socket_gid;
    std::fs::create_dir_all(&cfg.range_dir).expect("cannot create the range directory");
    std::fs::set_permissions(&cfg.range_dir, std::fs::Permissions::from_mode(0o755))
        .expect("cannot set the range directory's mode");

    let app = App::new(cfg.clone(), Docker::new(cfg.docker.clone()));
    app.recover().await;
    tracing::info!(
        "{} images allowed, at most {} instances",
        cfg.images.len(),
        cfg.max_instances
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

    let _ = std::fs::remove_file(&socket);
    let listener = tokio::net::UnixListener::bind(&socket)
        .unwrap_or_else(|e| panic!("cannot listen on {}: {e}", socket.display()));
    // Group-writable, and the group is the web server's: only it (and root)
    // can ask for an instance.
    std::os::unix::fs::chown(&socket, None, Some(gid)).expect("cannot set the socket's group");
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o660))
        .expect("cannot set the socket's mode");
    tracing::info!("listening on {}", socket.display());
    axum::serve(listener, router(app))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .expect("server error");
}

#[cfg(test)]
mod tests;
