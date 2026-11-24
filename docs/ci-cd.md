# CI/CD

Two GitHub Actions workflows live in `.github/workflows/`.

| Workflow | Runs on | Does |
| --- | --- | --- |
| `ci.yml` | pull requests, pushes to any branch except `main` and `dev` | `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, pack validation, installer syntax check |
| `deploy.yml` | pushes to `main` or `dev`, manual run | runs `ci.yml`, then deploys over SSH only if it passed: `main` to the `production` environment, `dev` to `development` |

Deploys of the same branch never overlap: a second push waits for the running one instead of cancelling it. A manual run from a branch other than `main` or `dev` does nothing.

## Branches

Two repository rulesets, with nobody on the bypass list, admins included:

- **`main`:** changes land only through a pull request. Direct pushes, force pushes and deleting the branch are blocked. The CI `check` and `image` jobs must pass, and review threads must be resolved, before merging. No approving review is required, since GitHub does not let you approve your own pull request. Raise it in the ruleset once someone else reviews.
- **`dev`:** ordinary pushes are allowed. Force pushes and deleting the branch are blocked.

The usual flow is push (or merge) into `dev`, which deploys to development, then a PR from `dev` into `main`, which deploys to production.

## What a deploy does

1. CI builds the Docker image from the commit that passed the checks.
2. It sends the image to the server over SSH (`docker load`), along with the small `deploy/bluegreen` bundle.
3. It runs `deploy.sh` on the server, which starts the new version beside the old one, waits for it to be healthy, switches traffic, and stops the old one. If anything fails before traffic moves, the live site is untouched.

The server needs no git checkout, Rust or build tools, only Docker. Details, rollback and server setup: [docker.md](docker.md).

CI also builds the image on every PR and runs it with the production hardening (read-only filesystem, no capabilities, fresh data volume) to check it starts, answers requests, runs as non-root and stops cleanly.

## Secrets

Each environment, `production` and `development` (Settings, Environments), has its own copy of these. Environment secrets are only given to the deploy job, each environment only accepts deploys from its own branch, and the environment is where you can add required reviewers if you want a manual approval before each deploy.

Use a separate deploy user for `development` (the starter value is `necropsy-dev`). The deploy state lives under the deploy user's home, so two environments sharing one user on one server would overwrite each other. On a shared server, also give the dev user its own `NECROPSY_PORT` and `NECROPSY_SUBNET` in its `deploy.env` (see [docker.md](docker.md)).

| Secret | Required | Value |
| --- | --- | --- |
| `DEPLOY_HOST` | yes | Server hostname or IP |
| `DEPLOY_PORT` | yes | SSH port, a number from 1 to 65535 |
| `DEPLOY_USER` | yes | The user that owns the service and the checkout |
| `DEPLOY_SSH_KEY` | yes | The private key, including the `-----BEGIN` and `-----END` lines |
| `DEPLOY_KNOWN_HOSTS` | yes | The server's host key line(s), see below |
| `DEPLOY_PATH` | no | Where the deploy files are unpacked on the server, relative to the deploy user's home unless absolute. Defaults to `necropsy-deploy`. Letters, digits, `.`, `_`, `/` and `-` only |

Optional **variable** (not a secret). Set it on each environment (Settings, Environments, Environment variables) so each checks its own site:

| Variable | Value |
| --- | --- |
| `DEPLOY_HEALTHCHECK_URL` | A URL such as `https://necropsy.urashyl.dev/healthz`. After deploying, the workflow requests it up to six times and fails the run if it never succeeds. |

The workflow fails early with the names of any missing secrets.

## Setting up the key and host key

Use a key made only for this. On your own machine:

```bash
ssh-keygen -t ed25519 -N '' -C necropsy-deploy -f necropsy_deploy
```

Do this once per environment. Put the contents of `necropsy_deploy` in `DEPLOY_SSH_KEY`, then delete the file. Add the public key to the deploy user's `~/.ssh/authorized_keys` on the server. The `restrict` prefix turns off forwarding and other extras:

```text
restrict ssh-ed25519 AAAA... necropsy-deploy
```

Get the host key for `DEPLOY_KNOWN_HOSTS` and check its fingerprint against the server before trusting it:

```bash
ssh-keyscan -p <port> -t ed25519 <host> | tee known_hosts_line
ssh-keygen -lf known_hosts_line
```

Compare that fingerprint with the one on the server (`ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub`). The workflow connects with `StrictHostKeyChecking=yes` and only this line, so it refuses to connect to anything else. For a non-default port the line starts with `[host]:port`, which `ssh-keyscan -p` produces.

## Server requirements

- Docker with the compose plugin, and `curl` (the deploy checks the site through the proxy).
- The deploy user in the `docker` group. That group is equivalent to root, so use an account made only for this, with the `restrict` option on its key as above. Rootless Docker is safer if you can use it.
- Your reverse proxy pointed at the loopback port, see [docker.md](docker.md).

## When it fails

- **Missing secrets:** the first step lists them.
- **Host key verification failed:** `DEPLOY_KNOWN_HOSTS` does not match the server, or the port is wrong. Regenerate it with `ssh-keyscan`.
- **Permission denied:** the public key is not in the deploy user's `authorized_keys`, or `DEPLOY_USER` is wrong.
- **"cannot talk to Docker":** the deploy user is not in the `docker` group, or the daemon is not running.
- **"did not become healthy":** the new version failed to start. The log run prints its last lines. The old version is still serving.
- **Image build errors:** shown in the build step of the run log, before anything touches the server.
