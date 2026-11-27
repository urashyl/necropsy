# Docker and blue-green deploys

This is the zero-downtime way to run necropsy, and what the CI/CD workflow deploys to. The systemd install in [deploy.md](deploy.md) still works for a simple single-host setup, but it restarts the service on every update, so there are a few seconds of downtime and no automatic rollback.

## How it works

```text
your reverse proxy ──► proxy (Caddy) ──► necropsy-blue    live
 (external, yours)                   └─► necropsy-green   idle or draining
```

Your own reverse proxy stays yours: it terminates TLS and forwards to one loopback port. Inside, a small Caddy container sits in front of two copies of the app, and one of them is live. A deploy:

1. builds the new version,
2. starts it as the idle color and waits for its health check,
3. points Caddy at it and reloads gracefully (requests in flight finish on the old color),
4. checks the site through the proxy, and goes back to the old color if that fails,
5. waits a short drain period, then stops the old color.

The old color is stopped, not deleted, so a rollback takes seconds. If the new color never becomes healthy, or Caddy rejects its config, or the site does not answer after the switch, the deploy stops and traffic stays exactly where it was. A bad release never takes the site down.

The image contains the binary, the packs, the [Learn](learn.md) modules and the workspace's [toolkit](toolkit.md) (downloaded by `toolkit/fetch.sh` in a build stage of its own, about 160 MB), so each image is a complete release. Building it needs network access to `registry.npmjs.org`, `github.com` and `release-assets.githubusercontent.com`; a changed download fails the build. The database lives in the `necropsy-data` Docker volume, shared by both colors.

## Who can do what

The deploy command is a root-owned program that one unprivileged user may run through sudo, without a password. It accepts `deploy <commit>`, `rollback` and `status`, and refuses everything else:

| | |
| --- | --- |
| `/usr/local/bin/necropsy-deploy` | owned by root, so the deploy user cannot change it |
| `/usr/local/lib/necropsy/` | the compose file, Caddyfile and settings template it uses, also root-owned |
| `/etc/necropsy/` | settings (`necropsy.env` for the app, `deploy.env` for deploy options), root-only |
| `/var/lib/necropsy/` | its state and a clone of the repository, root-only |
| `/etc/sudoers.d/necropsy-deploy` | lets the deploy user run that one program as root, without a password. It is the only passwordless sudo they have |

The deploy user is **not** in the `docker` group (that group is root-equivalent). Because it can start the program as root, the program trusts none of what it controls:

- **Which code:** `deploy` takes a full commit hash, and the program fetches the repository named in `/etc/necropsy/deploy.env` (https only) and refuses any commit that is not on the branch named there (`NECROPSY_BRANCH`, `main` unless the server was installed with `--branch dev`). The deploy user can choose between releases but cannot add one, nor change the branch.
- **Its arguments:** the sudo rule has no argument patterns (they differ between sudo implementations), so the program checks its own: a full 40-character hash for `deploy`, nothing for the others, and any other command is refused.
- **Which files:** the compose file and Caddyfile come from the root-owned install, never from the caller.
- **The environment:** it ignores the caller's environment entirely (`DOCKER_HOST`, `GIT_*`, `LD_PRELOAD` and the like) and starts again with a clean one.
- **Its own settings:** `deploy.env` is parsed as plain `KEY=VALUE`, never executed, and the program refuses it if anyone but root can write to it.
- **The containers:** they run with a read-only filesystem, no capabilities, no new privileges and a non-root user, whatever the image says.

Changing the program, compose file or settings means running the installer again as root. That is the point, but it also means those do not update through CI.

## Server setup, once

On a dedicated Debian or Ubuntu VM, [`deploy/install-site.sh`](range.md#setting-it-up) does all of this, and connects the site to the range VM that runs live-puzzle labs. The steps below are what it automates.

You need Docker with the compose plugin, `git`, `curl` and `flock` (util-linux), and a user for deployments that is not root and not in the `docker` group. As root, from a checkout of the repository:

```bash
deploy/bluegreen/install-system.sh --user deploy --repo https://github.com/urashyl/necropsy
```

That is a production server, which builds only commits on `main`. For the dev server, a separate host, add `--branch dev` so it builds only commits on `dev`:

```bash
deploy/bluegreen/install-system.sh --user deploy --repo https://github.com/urashyl/necropsy --branch dev
```

It checks the prerequisites, installs everything above, validates the sudo rule with `visudo` before installing it, and creates `/etc/necropsy/necropsy.env` from the example. Edit that file (base URL, SMTP) as root. Settings are read when a container starts, so a deploy is also how a settings change takes effect. Run the installer again after changing anything in the repository's `deploy/bluegreen/`, with the same options (it keeps your settings).

Then point your reverse proxy at the proxy container's loopback port, `8080` by default (`NECROPSY_PORT` in `/etc/necropsy/deploy.env`). With Caddy:

```caddy
necropsy.urashyl.dev {
    reverse_proxy 127.0.0.1:8080 {
        header_up X-Real-IP {remote_host}
    }
}
```

### A reverse proxy on another host

If your reverse proxy is not on this host (it is managed outside the VM, say), publish the proxy container on all addresses and trust that proxy, in `/etc/necropsy/deploy.env`:

```bash
NECROPSY_BIND=0.0.0.0          # or this host's own address
NECROPSY_EDGE_TRUSTED=10.0.0.5 # your reverse proxy's address
```

The next deploy recreates the proxy container on the new address. Traffic pauses for the second that takes, which happens only when the address or port changes. Then point your reverse proxy at `http://<this host>:8080`.

Port 8080 is now reachable by anything that can reach the host, over plain HTTP and around your proxy, so allow only your proxy to reach it. Docker writes its own iptables rules for published ports and they skip ufw and firewalld, so a ufw rule does not block it. Use a firewall outside the VM, or Docker's `DOCKER-USER` chain (replace `eth0` with the host's external interface):

```bash
iptables -I DOCKER-USER -i eth0 -p tcp --dport 8080 ! -s 10.0.0.5 -j DROP
```

That chain sees the port inside the container, which is always 8080, so keep `--dport 8080` even if you change `NECROPSY_PORT`. Make the rule persist across reboots the way your distribution does (`iptables-persistent`, `netfilter-persistent save`, or similar).

### Headers

Caddy passes the `Host` header through unchanged, which the app needs, and WebSocket upgrades (the workspace's request console) with no extra settings. The `X-Real-IP` line is what lets rate limits see the real visitor: the proxy container believes that header only from your proxy's address (the Docker gateway and loopback by default, `NECROPSY_EDGE_TRUSTED` in `deploy.env` to change it). Your proxy should always set it itself, as above, and never forward one from a visitor.

## Several stacks on one host

Production and dev normally have a VM each, with the default stack name, `necropsy`. A host can run more than one stack, as long as each has its own name. `install-system.sh --name necropsy-dev` installs a second one next to the first:

```bash
deploy/bluegreen/install-system.sh --name necropsy-dev --user deploy-dev --repo https://github.com/urashyl/necropsy --branch dev
```

Everything that could clash follows the name: the program (`necropsy-dev-deploy`), its settings (`/etc/necropsy-dev/`), state, sudo rule, containers (`necropsy-dev-blue`, `-green`, `-proxy`), the data volume and the image repository (`necropsy-dev:<commit>`). The default name keeps every path as it was. Give each stack its own port and subnet in its `deploy.env`, and its own `RANGE_TENANT` if it uses the range:

```bash
# /etc/necropsy-dev/deploy.env
NECROPSY_PORT=8081
NECROPSY_SUBNET=172.30.31.0/24
```

Use a different deploy user per stack, so write access to one branch cannot run the other's deploy, and set `DEPLOY_PROGRAM` on its GitHub environment to its program's name ([ci-cd.md](ci-cd.md)). The firewall rule in the proxy section covers every stack, since the port it names is the container's own.

## Settings that differ in Docker

These are fixed by the container and cannot be overridden in `necropsy.env`:

| Setting | In the container |
| --- | --- |
| `NECROPSY_ADDR` | `0.0.0.0:8080` |
| `NECROPSY_CLIENT_IP_HEADER` | `X-Real-IP` |
| `NECROPSY_TRUSTED_PROXIES` | the containers' private subnet, so only the proxy container can set it |
| `NECROPSY_PACKS`, `NECROPSY_DB` | the packs baked into the image, and `/data/necropsy.db` |
| `NECROPSY_LEARN`, `NECROPSY_TOOLKIT` | `/app/learn` and `/app/toolkit`, baked into the image |
| server secret | `/data/server-secret` in the data volume, made on first start (see [generators.md](generators.md#the-server-secret)); `NECROPSY_SERVER_SECRET_FILE` moves it |

Deploy options (bind address, port, subnet, drain time, repository, branch) go in `/etc/necropsy/deploy.env`, see `deploy/bluegreen/deploy.env.example`. Quote a value in `necropsy.env` that contains `$` with single quotes, since Compose expands `$` in environment files.

## Deploying

CI does this on every push to `main`, and to `dev` on the dev server ([ci-cd.md](ci-cd.md)). By hand, as the deploy user (or as root without `sudo`):

```bash
sudo necropsy-deploy deploy "$(git rev-parse origin/main)"   # a full 40-character hash on main
sudo necropsy-deploy status        # live color, images, health
sudo necropsy-deploy rollback      # back to the previous release
docker logs -f necropsy-blue       # or necropsy-green, as root
```

The server builds the image itself, so a deploy takes a couple of minutes, during which the live version keeps serving. The first build on a machine compiles every dependency; later ones reuse them from BuildKit's cache on that machine (the downloaded crates and the compiled `target` directory are cache mounts in the `Dockerfile`), so only this repository's own crates are rebuilt. Clearing the Docker build cache, or the weekly prune of unused cache, makes the next build a full one. On the very first deploy the app creates the admin account and prints its credentials once:

```bash
docker logs necropsy-blue 2>&1 | grep -A4 'first run'
```

Deploys are serialized with a lock, so two cannot overlap.

## Moving from the systemd install

Stop the old service so the database is not in use, copy it into the volume, then deploy:

```bash
systemctl --user disable --now necropsy      # as the user that ran it
docker volume create necropsy-data
docker run --rm --user 0 -v necropsy-data:/data -v /home/USER/.local/state/necropsy:/old:ro busybox \
  sh -c 'cp /old/necropsy.db* /data/ && chown 65532:65532 /data/necropsy.db*'
```

Copy your settings from `~/.config/necropsy/necropsy.env` to `/etc/necropsy/necropsy.env`. The old service and the proxy container cannot share a port, so stop one before starting the other or change `NECROPSY_PORT`.

## Database changes

During a switch both versions run against the same database for a short time, and a rollback runs the old version against a database the new one has used. So a release must keep working with the previous schema: add columns and tables in one release, and remove or rename things only in a later one.

## Files

| File | |
| --- | --- |
| `Dockerfile` | Builds the image. Distroless, non-root, read-only friendly, with a built-in health check. |
| `deploy/bluegreen/deploy.sh` | The deploy program (a template: the installer bakes in the paths). |
| `deploy/bluegreen/install-system.sh` | Installs it as a root-owned command with the sudo rule. |
| `deploy/bluegreen/compose.yaml` | Caddy and the two colors, with hardening. |
| `deploy/bluegreen/Caddyfile` | Caddy's config. It has no access log, since email links carry tokens in the query string. |
| `deploy/bluegreen/test/run.sh` | Tests the program and installer against a fake Docker, no daemon needed. CI runs it as root so the tests of the real install run too. |
