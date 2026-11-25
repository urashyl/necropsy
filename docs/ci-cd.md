# CI/CD

Two GitHub Actions workflows live in `.github/workflows/`.

| Workflow | Runs on | Does |
| --- | --- | --- |
| `ci.yml` | pull requests, pushes to any branch except `main` and `dev` | `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, pack validation, script syntax and the installer tests; and with real Docker: the image, the deploy program (two deploys under load, rollback, a second stack beside the first), and the range (every lab comes up locked down, passes its own test, is reaped) |
| `deploy.yml` | pushes to `main` or `dev`, manual run | runs `ci.yml`, then deploys over SSH only if it passed: `main` to the `production` environment, `dev` to `development` |

Deploys of the same branch never overlap: a second push waits for the running one instead of cancelling it. A manual run from a branch other than `main` or `dev` does nothing.

## Branches

Two repository rulesets, with nobody on the bypass list, admins included:

- **`main`:** changes land only through a pull request. Direct pushes, force pushes and deleting the branch are blocked. The CI `check` and `image` jobs must pass, and review threads must be resolved, before merging. No approving review is required, since GitHub does not let you approve your own pull request. Raise it in the ruleset once someone else reviews.
- **`dev`:** ordinary pushes are allowed. Force pushes and deleting the branch are blocked.

The usual flow is push (or merge) into `dev`, which deploys to development, then a PR from `dev` into `main`, which deploys to production.

## What a deploy does

CI connects to the server over SSH and runs one command: `sudo -n <program> deploy <commit>`, where the program is `necropsy-deploy` for `main` and `necropsy-dev-deploy` for `dev` (see [One VM for both](#one-vm-for-both-environments)). That is a root-owned program the deploy user may run without a password, and the only thing they may ([docker.md](docker.md) explains why it is safe to). The program fetches the repository itself, refuses a commit that is not on the branch it was installed for (`main` on the production server, `dev` on the dev server), builds the image, starts the new version beside the old one, waits for it to be healthy, and only then switches traffic. If anything fails before traffic moves, the live site is untouched.

CI sends no image and no files, only the commit hash. The server needs Docker and git, not Rust.

For a stack that runs live-puzzle labs ([range.md](range.md)), the same command also ships them, before any traffic moves: it builds the commit's lab images, rewrites that stack's lab allowlist to point at them, restarts that stack's spawner, and starts a real lab of every image to check it is locked down and answers. If any of that fails, the previous allowlist is put back and the site is untouched. A new lab is therefore just a commit: its `range/<name>/` directory, its entry in `range/images.toml` and its pack. The spawner *program* itself is replaced only by a production deploy (see below).

CI also builds the image on every PR and runs it with the production hardening (read-only filesystem, no capabilities, fresh data volume), and a separate end-to-end job installs the deploy program, deploys twice with real Docker while requesting the site continuously, and checks that not one request failed during the switch.

## Secrets

Each environment, `production` and `development` (Settings, Environments), has its own copy of these. Environment secrets are only given to the deploy job, each environment only accepts deploys from its own branch, and the environment is where you can add required reviewers if you want a manual approval before each deploy.

Production and dev each have their own deploy program, so they can run on separate hosts or share one VM; the next section covers sharing.

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

## One VM for both environments

Production and dev can share one VM. They are then two independent stacks, made by `deploy/install-vm.sh --dev-domain ...` (or `deploy/bluegreen/install-system.sh --name necropsy-dev` by hand, see [docker.md](docker.md#several-stacks-on-one-host)):

| | production | dev |
| --- | --- | --- |
| branch built | `main` | `dev` |
| deploy program (what CI runs) | `necropsy-deploy` | `necropsy-dev-deploy` |
| SSH / sudo user | `deploy` | `deploy-dev` |
| containers, volume | `necropsy-blue`, `necropsy-data`, ... | `necropsy-dev-blue`, `necropsy-dev-data`, ... |
| site port on the VM | 8080 | 8081 |
| spawner, labs | `necropsy-spawner`, `/etc/necropsy/images.toml` | `necropsy-dev-spawner`, `/etc/necropsy-dev/images.toml` |
| domain | `necropsy.urashyl.dev` | e.g. `necropsy-dev.urashyl.dev` |

Each environment's secrets differ only in `DEPLOY_USER` and `DEPLOY_SSH_KEY`: `DEPLOY_HOST`, `DEPLOY_PORT` and `DEPLOY_KNOWN_HOSTS` are the same VM's. The two users each have a sudo rule for their own program only, so **a push to `dev`, which accepts direct pushes, cannot run production's deploy**, and the dev program refuses any commit that is not on `dev`.

Things to know about sharing:

- **The VM's capacity is shared.** Each stack has its own lab limit (`RANGE_MAX_INSTANCES`), and `install-vm.sh` splits what the VM holds two thirds to production, one third to dev. The limits are independent, so keep their sum within what [range.md](range.md#how-many-players-it-holds) says the VM holds.
- **The spawner program is shared and runs as root, so only production replaces it.** A production deploy builds it from the commit on `main` (reviewed through a pull request) and restarts both spawners. A dev deploy builds the dev labs (sandboxed containers, like any other) but never replaces that program, so write access to `dev` does not reach root on the VM. A change to the spawner is therefore live in production before dev, and can only be tried on dev by merging it to `main`.
- **Lab images are separate per stack** (`necropsy-range/<stack>/<lab>`), and one stack's cleanup never removes the other's.
- **One firewall rule** keeps both sites' ports reachable only from your reverse proxy.
- **A dev server on its own host** is still possible: install it with the default name, and set the `DEPLOY_PROGRAM` variable on the `development` environment to `necropsy-deploy`.

## Setting up the key and host key

Use a key made only for this. On your own machine:

```bash
ssh-keygen -t ed25519 -N '' -C necropsy-deploy -f necropsy_deploy
```

Do this once per environment, with a different key each (on one VM, they belong to different users). Put the contents of `necropsy_deploy` in `DEPLOY_SSH_KEY`, then delete the file. Add the public key to that environment's deploy user's `~/.ssh/authorized_keys` on the server (`deploy` for production, `deploy-dev` for dev). The `restrict` prefix turns off forwarding and other extras:

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

On a dedicated VM, run `deploy/install-vm.sh` once ([range.md](range.md#setting-it-up)): it installs Docker, the deploy programs, the spawners and the firewall rule for each environment. To do the site alone by hand, as described in [docker.md](docker.md): Docker with the compose plugin, `git`, `curl`, and the installer run as root with `--user` set to the deploy user. The deploy user's SSH key should be one made for this (above), and the user must not be in the `docker` group, which would make the sudo restriction pointless; the installer refuses that.

## When it fails

- **Missing secrets:** the first step lists them.
- **Host key verification failed:** `DEPLOY_KNOWN_HOSTS` does not match the server, or the port is wrong. Regenerate it with `ssh-keyscan`.
- **Permission denied:** the public key is not in the deploy user's `authorized_keys`, or `DEPLOY_USER` is wrong.
- **"sudo: a password is required":** the sudo rule is missing or does not match. Re-run the installer as root, and check that `DEPLOY_USER` is the user it was given in `--user`.
- **"commit ... is not on main":** the commit was pushed to another branch, or `main` was rewritten. Only commits on `main` are built (on `dev` for the dev program).
- **"sudo: necropsy-dev-deploy: command not found" or a password prompt:** the program name is wrong for the user. `main` runs `necropsy-deploy` as the production user, `dev` runs `necropsy-dev-deploy` as the dev user; check `DEPLOY_USER` on each environment.
- **"the labs failed their test. The site was not changed":** a lab in that commit did not come up locked down, or does not answer. The log above it says which check failed, and the previous allowlist is back in place. Run `deploy/range/ci-test.sh` locally to reproduce.
- **"did not become healthy":** the new version failed to start. The log run prints its last lines. The old version is still serving.
- **Build errors:** shown in the deploy step of the run log. The live version keeps serving.
