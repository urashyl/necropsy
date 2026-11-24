# CI/CD

Two GitHub Actions workflows live in `.github/workflows/`.

| Workflow | Runs on | Does |
| --- | --- | --- |
| `ci.yml` | pull requests, pushes to any branch except `main` | `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, pack validation, installer syntax check |
| `deploy.yml` | pushes to `main`, manual run | runs `ci.yml`, then deploys over SSH only if it passed |

Deploys never overlap: a second push waits for the running one instead of cancelling it. A manual run from a branch other than `main` does nothing.

## What a deploy does

CI connects to the server over SSH and runs one command: `sudo -n necropsy-deploy deploy <commit>`. That is a root-owned program the deploy user may run without a password, and nothing else ([docker.md](docker.md) explains why it is safe to). The program fetches the repository itself, refuses a commit that is not on `main`, builds the image, starts the new version beside the old one, waits for it to be healthy, and only then switches traffic. If anything fails before traffic moves, the live site is untouched.

CI sends no image and no files, only the commit hash. The server needs Docker and git, not Rust.

CI also builds the image on every PR and runs it with the production hardening (read-only filesystem, no capabilities, fresh data volume), and a separate end-to-end job installs the deploy program, deploys twice with real Docker while requesting the site continuously, and checks that not one request failed during the switch.

## Secrets

Create these in the repository under Settings, Environments, `production` (create the environment first). Environment secrets are only given to the deploy job, and the environment is where you can add required reviewers if you want a manual approval before each deploy.

| Secret | Required | Value |
| --- | --- | --- |
| `DEPLOY_HOST` | yes | Server hostname or IP |
| `DEPLOY_PORT` | yes | SSH port, a number from 1 to 65535 |
| `DEPLOY_USER` | yes | The user that owns the service and the checkout |
| `DEPLOY_SSH_KEY` | yes | The private key, including the `-----BEGIN` and `-----END` lines |
| `DEPLOY_KNOWN_HOSTS` | yes | The server's host key line(s), see below |

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

Set the server up once as described in [docker.md](docker.md): Docker with the compose plugin, `git`, `curl`, and the installer run as root with `--user` set to the deploy user. The deploy user's SSH key should be one made for this (above), and the user must not be in the `docker` group, which would make the sudo restriction pointless; the installer refuses that.

## When it fails

- **Missing secrets:** the first step lists them.
- **Host key verification failed:** `DEPLOY_KNOWN_HOSTS` does not match the server, or the port is wrong. Regenerate it with `ssh-keyscan`.
- **Permission denied:** the public key is not in the deploy user's `authorized_keys`, or `DEPLOY_USER` is wrong.
- **"sudo: a password is required":** the sudo rule is missing or does not match. Re-run the installer as root, and check that `DEPLOY_USER` is the user it was given in `--user`.
- **"commit ... is not on main":** the commit was pushed to another branch, or `main` was rewritten. Only commits on `main` are built.
- **"did not become healthy":** the new version failed to start. The log run prints its last lines. The old version is still serving.
- **Build errors:** shown in the deploy step of the run log. The live version keeps serving.
