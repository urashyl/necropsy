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

The image contains the binary and the packs, so each image is a complete release. The database lives in the `necropsy-data` Docker volume, shared by both colors.

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

Caddy passes the `Host` header through unchanged, which the app needs. The `X-Real-IP` line is what lets rate limits see the real visitor: the proxy container believes that header only from your proxy's address (the Docker gateway and loopback by default, `NECROPSY_EDGE_TRUSTED` in `deploy.env` to change it). Your proxy should always set it itself, as above, and never forward one from a visitor.

## Settings that differ in Docker

These are fixed by the container and cannot be overridden in `necropsy.env`:

| Setting | In the container |
| --- | --- |
| `NECROPSY_ADDR` | `0.0.0.0:8080` |
| `NECROPSY_CLIENT_IP_HEADER` | `X-Real-IP` |
| `NECROPSY_TRUSTED_PROXIES` | the containers' private subnet, so only the proxy container can set it |
| `NECROPSY_PACKS`, `NECROPSY_DB` | the packs baked into the image, and `/data/necropsy.db` |

Deploy options (port, subnet, drain time, repository, branch) go in `/etc/necropsy/deploy.env`, see `deploy/bluegreen/deploy.env.example`. Quote a value in `necropsy.env` that contains `$` with single quotes, since Compose expands `$` in environment files.

## Deploying

CI does this on every push to `main`, and to `dev` on the dev server ([ci-cd.md](ci-cd.md)). By hand, as the deploy user (or as root without `sudo`):

```bash
sudo necropsy-deploy deploy "$(git rev-parse origin/main)"   # a full 40-character hash on main
sudo necropsy-deploy status        # live color, images, health
sudo necropsy-deploy rollback      # back to the previous release
docker logs -f necropsy-blue       # or necropsy-green, as root
```

The server builds the image itself, so a deploy takes a couple of minutes, during which the live version keeps serving. On the very first deploy the app creates the admin account and prints its credentials once:

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
