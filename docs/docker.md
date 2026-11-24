# Docker and blue-green deploys

This is the zero-downtime way to run necropsy, and what the CI/CD workflow deploys to. The systemd install in [deploy.md](deploy.md) still works for a simple single-host setup, and restarts the service on each update, so there is a few seconds of downtime and no automatic rollback.

## How it works

```text
your reverse proxy ──► proxy ──► necropsy-blue    live
 (nginx, Caddy, ...)    (nginx)   necropsy-green   idle or draining
```

Both colors run the same image build, one at a time as the live one. A deploy:

1. starts the idle color from the new image,
2. waits for its health check to pass,
3. points the proxy at it and reloads the proxy gracefully (requests in flight finish on the old color),
4. checks the site through the proxy, and goes back to the old color if that fails,
5. waits a short drain period, then stops the old color.

The old color is stopped, not deleted, so `deploy.sh --rollback` brings the previous release back in seconds. If the new color never becomes healthy, or the proxy rejects its config, or the site does not answer after the switch, the deploy stops and traffic stays exactly where it was. A bad release never takes the site down.

The image contains the binary and the packs, so each image is a complete release. Settings come from `~/.config/necropsy/necropsy.env`, the same file as the systemd install. The database lives in the `necropsy-data` Docker volume, shared by both colors.

## Server setup, once

You need Docker with the compose plugin, and the deploy user in the `docker` group. Membership in that group is equivalent to root on the machine, so use a user made for this, with the deploy key restricted as described in [ci-cd.md](ci-cd.md). Rootless Docker avoids that, and works with this setup since it only publishes a port above 1024.

Then point your reverse proxy at the proxy container's loopback port (`8080` by default, `NECROPSY_PORT` to change it). For nginx:

```nginx
location / {
    proxy_pass http://127.0.0.1:8080;
    proxy_set_header Host $host;
    proxy_set_header X-Real-IP $remote_addr;
}
```

The `Host` header must be passed through, and your proxy must always set `X-Real-IP` itself. The proxy container believes that header only from your proxy's address, so rate limits see the real client.

The first deploy creates `~/.config/necropsy/necropsy.env` from `.env.example` with everything commented out. Edit it (base URL, SMTP) and deploy again. Settings are read when a container starts, so a deploy is also how a settings change takes effect.

## Settings that differ in Docker

These are fixed by the container and cannot be overridden in `necropsy.env`:

| Setting | In the container |
| --- | --- |
| `NECROPSY_ADDR` | `0.0.0.0:8080` |
| `NECROPSY_CLIENT_IP_HEADER` | `X-Real-IP` |
| `NECROPSY_TRUSTED_PROXIES` | the containers' private subnet, so only the proxy container can set it |
| `NECROPSY_PACKS`, `NECROPSY_DB` | the packs baked into the image, and `/data/necropsy.db` |

Deploy-time options (port, subnet, drain time) go in `~/.config/necropsy/deploy.env`, see `deploy/bluegreen/deploy.env.example`. Quote a value that contains `$` with single quotes, since Compose expands `$` in environment files.

## Deploying

CI does this on every push to `main` ([ci-cd.md](ci-cd.md)). By hand, from a checkout:

```bash
docker build -t necropsy:manual .
deploy/bluegreen/deploy.sh necropsy:manual
```

```bash
deploy/bluegreen/deploy.sh --status      # live color, images, health
deploy/bluegreen/deploy.sh --rollback    # back to the previous release
docker logs -f necropsy-blue             # or necropsy-green
```

On the very first deploy the server creates the admin account and prints its credentials once:

```bash
docker logs necropsy-blue 2>&1 | grep -A4 'first run'
```

Deploys are serialized with a lock, so two cannot overlap.

## Moving from the systemd install

Stop the old service so the database is not in use, copy it into the volume, then deploy:

```bash
systemctl --user disable --now necropsy
docker volume create necropsy-data
docker run --rm --user 0 -v necropsy-data:/data -v "$HOME/.local/state/necropsy":/old:ro busybox \
  sh -c 'cp /old/necropsy.db* /data/ && chown 65532:65532 /data/necropsy.db*'
```

The systemd service and the proxy container cannot both use the same port, so stop one before starting the other or set `NECROPSY_PORT`.

## Database changes

During a switch both versions run against the same database for a short time, and a rollback runs the old version against a database the new one has used. So a release must keep working with the previous schema: add columns and tables in one release, and remove or rename things only in a later one.

## Files

| File | |
| --- | --- |
| `Dockerfile` | Builds the image. Distroless, non-root, read-only friendly, with a built-in health check. |
| `deploy/bluegreen/compose.yaml` | The proxy and the two colors, with hardening (read-only root, no capabilities, no new privileges). |
| `deploy/bluegreen/nginx.conf` | The proxy's config. Logs paths only, never query strings, since email links carry tokens. |
| `deploy/bluegreen/deploy.sh` | Deploy, rollback and status. |
| `deploy/bluegreen/test/run.sh` | Tests the switching logic against a fake Docker, no daemon needed. CI runs it. |
