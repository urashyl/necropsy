# CI/CD

Two GitHub Actions workflows live in `.github/workflows/`.

| Workflow | Runs on | Does |
| --- | --- | --- |
| `ci.yml` | pull requests, pushes to any branch except `main` and `dev` | `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, pack validation, script syntax and the installer tests; and with real Docker: the image, the deploy program (two deploys under load, rollback), and the range (the real spawner with two tenants: every lab comes up locked down, passes its own test, is reaped, and tenants cannot touch each other) |
| `deploy.yml` | pushes to `main` or `dev`, manual run | runs `ci.yml`, then deploys over SSH only if it passed: `main` to the `production` environment, `dev` to `development` |

Deploys of the same branch never overlap: a second push waits for the running one instead of cancelling it. A manual run from a branch other than `main` or `dev` does nothing.

## Branches

Two repository rulesets, with nobody on the bypass list, admins included:

- **`main`:** changes land only through a pull request. Direct pushes, force pushes and deleting the branch are blocked. The CI `check` and `image` jobs must pass, and review threads must be resolved, before merging. No approving review is required, since GitHub does not let you approve your own pull request. Raise it in the ruleset once someone else reviews.
- **`dev`:** ordinary pushes are allowed. Force pushes and deleting the branch are blocked.

The usual flow is push (or merge) into `dev`, which deploys to development, then a PR from `dev` into `main`, which deploys to production.

## What a deploy does

CI connects to the server over SSH and runs one command: `sudo -n necropsy-deploy deploy <commit>`. That is a root-owned program the deploy user may run without a password, and the only thing they may ([docker.md](docker.md) explains why it is safe to). The program fetches the repository itself, refuses a commit that is not on the branch it was installed for (`main` on the production server, `dev` on the dev server), builds the image, starts the new version beside the old one, waits for it to be healthy, and only then switches traffic. If anything fails before traffic moves, the live site is untouched.

CI sends no image and no files, only the commit hash. The server needs Docker and git, not Rust.

For a site that runs live-puzzle labs ([range.md](range.md)), the same command also ships them, before any traffic moves: it builds the commit's lab images, pushes the ones the range VM does not have, sends the range this site's lab allowlist, and has the range start every lab and check it is locked down and answers. If any of that fails, the range's previous allowlist is put back and the site is untouched. A new lab is therefore just a commit: its `range/<name>/` directory, its entry in `range/images.toml` and its pack. The spawner *program* on the range VM is not updated by site deploys: it runs as root there, so a site, least of all the dev branch, must not be able to change it. It updates itself instead, from `main` only (a timer on the range VM, see [range.md](range.md)), so a merge to `main` that changes the spawner reaches the range within minutes with nothing to run by hand.

CI also builds the image on every PR and runs it with the production hardening (read-only filesystem, no capabilities, fresh data volume). A separate end-to-end job installs the deploy program, deploys twice with real Docker while requesting the site continuously, and checks that not one request failed during the switch. A `range` job runs the real spawner and labs against real Docker.

## Secrets

Each environment, `production` and `development` (Settings, Environments), has its own copy of these. Environment secrets are only given to the deploy job, each environment only accepts deploys from its own branch, and the environment is where you can add required reviewers if you want a manual approval before each deploy.

Production and dev are separate VMs, each with its own install of the deploy program (`deploy/install-site.sh`: on `main` for production, with `--branch dev` for dev), so each environment's `DEPLOY_HOST`, `DEPLOY_USER` and key are its own. A third VM, the range, runs the live-puzzle labs for both ([range.md](range.md)). CI never connects to it: it is reached only from the sites' deploys and web servers, with a token per site.

| Secret | Required | Value |
| --- | --- | --- |
| `DEPLOY_HOST` | yes | Server hostname or IP |
| `DEPLOY_PORT` | yes | SSH port, a number from 1 to 65535 |
| `DEPLOY_USER` | yes | The user that owns the service and the checkout |
| `DEPLOY_SSH_KEY` | yes | The private key, including the `-----BEGIN` and `-----END` lines |
| `DEPLOY_KNOWN_HOSTS` | yes | The server's host key line(s), see below |

Optional **variable** (not a secret). Set it on each environment (Settings, Environments, Environment variables) so each checks its own site:

| Variable | Value |
| --- | --- |
| `DEPLOY_HEALTHCHECK_URL` | A URL such as `https://necropsy.urashyl.dev/healthz`. After deploying, the workflow requests it up to six times and fails the run if it never succeeds. |

The workflow fails early with the names of any missing secrets.

## The range VM

Container-backed puzzles run on the range VM, shared by production and dev, and need nothing in GitHub: no secrets, no workflow. Each site's deploy talks to it directly, with that site's token (`/etc/necropsy/range.token` on the site VM, made by `deploy/install-range.sh` on the range VM). The token says which **tenant** the site is (`prod` or `dev`), so:

- a push to `dev` deploys dev's labs and allowlist and can only ever touch dev's tenant: it cannot see, stop or replace production's labs, and cannot push an image under production's name;
- each tenant has its own cap on labs and on their memory, so dev being busy cannot take room from production;
- the dev branch accepts direct pushes, and the labs it ships are sandboxed containers on a VM with nothing else on it. The spawner program that runs them is not something a deploy can change.

## Setting up the key and host key

Use a key made only for this. On your own machine:

```bash
ssh-keygen -t ed25519 -N '' -C necropsy-deploy -f necropsy_deploy
```

Do this once per environment, with a different key each. Put the contents of `necropsy_deploy` in `DEPLOY_SSH_KEY`, then delete the file. Add the public key to that environment's deploy user's `~/.ssh/authorized_keys` on the server (`deploy`, on each environment's own VM). The `restrict` prefix turns off forwarding and other extras:

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

On each site's VM, run `deploy/install-site.sh` once ([range.md](range.md#setting-it-up)): it installs Docker, the deploy program, the settings, the connection to the range and the firewall rule. To do it by hand, as described in [docker.md](docker.md): Docker with the compose plugin, `git`, `curl`, and the installer run as root with `--user` set to the deploy user. The deploy user's SSH key should be one made for this (above), and the user must not be in the `docker` group, which would make the sudo restriction pointless; the installer refuses that.

## When it fails

- **Missing secrets:** the first step lists them.
- **Host key verification failed:** `DEPLOY_KNOWN_HOSTS` does not match the server, or the port is wrong. Regenerate it with `ssh-keyscan`.
- **Permission denied:** the public key is not in the deploy user's `authorized_keys`, or `DEPLOY_USER` is wrong.
- **"sudo: a password is required":** the sudo rule is missing or does not match. Re-run the installer as root, and check that `DEPLOY_USER` is the user it was given in `--user`.
- **"commit ... is not on main":** the commit was pushed to another branch, or `main` was rewritten. Only commits on `main` are built (on `dev` for the dev program).
- **"the labs failed their test. The site was not changed":** a lab in that commit did not come up locked down, or does not answer. The range's report above it says which check failed, and its previous allowlist is back in place. Run `deploy/range/ci-test.sh` to reproduce it against real Docker.
- **"the range refused the allowlist" or "did not list its images (401)":** the token in `/etc/necropsy/range.token` is not the one the range made for this site's tenant, or the range's firewall does not let this VM in. Check with `curl http://<range>:9400/healthz`.
- **"The range is full right now" on the site:** the tenant is at its lab limit or its memory budget. Players can retry; raise the limits in `/etc/necropsy-range/tenants.toml` if it is routine.
- **"did not become healthy":** the new version failed to start. The log run prints its last lines. The old version is still serving.
- **Build errors:** shown in the deploy step of the run log. The live version keeps serving.
