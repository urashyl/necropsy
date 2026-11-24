# CI/CD

Two GitHub Actions workflows live in `.github/workflows/`.

| Workflow | Runs on | Does |
| --- | --- | --- |
| `ci.yml` | pull requests, pushes to any branch except `main` | `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, pack validation, installer syntax check |
| `deploy.yml` | pushes to `main`, manual run | runs `ci.yml`, then deploys over SSH only if it passed |

Deploys never overlap: a second push waits for the running one instead of cancelling it. A manual run from a branch other than `main` does nothing.

## What a deploy does

The workflow connects to your server over SSH and runs, in the server's checkout:

```text
git fetch origin main <sha>
git checkout main
git reset --hard <sha>
deploy/install.sh
```

`install.sh` builds on the server, installs the binary and packs, and restarts the user service. Settings (`necropsy.env`) and the database are never touched. See [deploy.md](deploy.md).

The server's checkout is managed by CI, so keep local edits out of tracked files there (`reset --hard` discards them). Untracked and ignored files are left alone. Put settings in `~/.config/necropsy/necropsy.env`.

## Secrets

Create these in the repository under Settings, Environments, `production` (create the environment first). Environment secrets are only given to the deploy job, and the environment is where you can add required reviewers if you want a manual approval before each deploy.

| Secret | Required | Value |
| --- | --- | --- |
| `DEPLOY_HOST` | yes | Server hostname or IP |
| `DEPLOY_PORT` | yes | SSH port, a number from 1 to 65535 |
| `DEPLOY_USER` | yes | The user that owns the service and the checkout |
| `DEPLOY_SSH_KEY` | yes | The private key, including the `-----BEGIN` and `-----END` lines |
| `DEPLOY_KNOWN_HOSTS` | yes | The server's host key line(s), see below |
| `DEPLOY_PATH` | no | The checkout on the server, relative to the deploy user's home unless absolute. Defaults to `necropsy` |

Optional repository **variable** (not a secret), under Settings, Secrets and variables, Actions, Variables:

| Variable | Value |
| --- | --- |
| `DEPLOY_HEALTHCHECK_URL` | A URL such as `https://necropsy.urashyl.dev/healthz`. After deploying, the workflow requests it up to six times and fails the run if it never succeeds. |

The workflow fails early with the names of any missing secrets.

## Setting up the key and host key

Use a key made only for this. On your own machine:

```bash
ssh-keygen -t ed25519 -N '' -C necropsy-deploy -f necropsy_deploy
```

Put the contents of `necropsy_deploy` in `DEPLOY_SSH_KEY`, then delete the file. Add the public key to the deploy user's `~/.ssh/authorized_keys` on the server. The `restrict` prefix turns off forwarding and other extras:

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

- A checkout of the repository, on `main`, with `origin` pointing at it (the repository is public, so HTTPS needs no credentials).
- `cargo` for the deploy user, installed through rustup, which the workflow picks up from `~/.cargo/env`.
- The service installed once by hand with `deploy/install.sh`, and lingering enabled, so the user manager is available to SSH sessions.

## When it fails

- **Missing secrets:** the first step lists them.
- **Host key verification failed:** `DEPLOY_KNOWN_HOSTS` does not match the server, or the port is wrong. Regenerate it with `ssh-keyscan`.
- **Permission denied:** the public key is not in the deploy user's `authorized_keys`, or `DEPLOY_USER` is wrong.
- **Build or install errors:** shown in the run log, from `install.sh`. The previous binary keeps running if the build fails, because the install only replaces it after a successful build.
