# The range: real servers for live puzzles

A [live puzzle](live.md) can be backed by a real server instead of simulated routes. Each player gets their own container, started just for them, running real software: a PHP site with a working webshell, a vulnerable app, whatever the puzzle needs. They talk to it at their own `https://<label>.necropsy.urashyl.dev` link, the same as any live puzzle.

```text
 player ──https──► your proxy ──► necropsy-web ──unix socket──► container   (no network)
                                       │
                                       └─unix socket─► necropsy-spawner ──► docker
```

## Security model

The point is to let players run things (a webshell executes their commands) without that touching anything else. The layers:

- **No network.** Containers start with `--network=none`. They cannot reach the internet, the host, the site or each other. The only way in is a unix socket, in a directory mounted from the host, that the web server connects to. There is nothing to firewall and no lateral movement between players.
- **Locked down.** Read-only root, all capabilities dropped, `no-new-privileges`, an unprivileged user, a pid limit, hard memory and cpu limits (swap included), a small `noexec` tmpfs as the only writable place, no logs kept.
- **The web server never holds the Docker socket.** That would be root on the host. It talks to `necropsy-spawner`, a small separate program, which can only start images on the allowlist in `/etc/necropsy/images.toml`, with the limits written there, and with only two environment variables (`NECROPSY_FLAG`, `NECROPSY_INSTANCE`) that it checks. A compromised site can ask for a lab; it cannot start an arbitrary container or mount anything.
- **Bounded.** `max_instances` across everything, three per account, and a captcha ([Turnstile](accounts.md#captcha)) before an instance is created. An instance is stopped when its time is up.
- **Disposable.** Every instance is a fresh container from the image. When it ends, it and its socket directory are deleted. Nothing a player does persists.

What this is **not** is a defence against a kernel or container-runtime escape. Containers share the host kernel. For a public site, keep Docker and the kernel patched, run this on a VM that holds nothing but necropsy (as below), and consider gVisor (`runsc`) or Kata as the runtime if you want more: the spawner will use whatever default runtime Docker has configured.

An instance lives for the puzzle's `ttl_minutes`, and is **extended while it is used**: when less than half is left and a request arrives, it gets a full term again, up to 8 hours from when it started. An idle instance goes away on its own.

## Setting it up

On a dedicated VM (Debian or Ubuntu), as root, from a checkout of the repository, one script does all of it, the site as well as the range:

```bash
sudo deploy/install-vm.sh \
  --domain necropsy.urashyl.dev \
  --proxy-ip 192.168.100.5 \
  --dev-domain necropsy-dev.urashyl.dev \
  --turnstile-site-key 0x4AAAAAAA... --turnstile-secret-file /root/turnstile-secret \
  --smtp-host smtp.example.com --smtp-user mailer --smtp-from "necropsy <no-reply@example.com>"
```

`--proxy-ip` is the address your reverse proxy connects from. `--dev-domain` is optional: it also installs a dev stack on the same VM, as a second, independent environment ([ci-cd.md](ci-cd.md#one-vm-for-both-environments)). Add `--dry-run` first to see everything it would do. Secrets are never options (they would land in `ps` and your shell history): the Turnstile secret goes in a file, and the SMTP password in `NECROPSY_SMTP_PASSWORD`. Run `deploy/install-vm.sh --help` for the rest.

What it does, in order:

1. Installs what is missing: Docker from Docker's own apt repository, plus git, curl, iptables and friends.
2. Builds the spawner inside a throwaway Rust container, as a static binary, so the VM needs no Rust and the result does not depend on its libc. It is installed once, at `/usr/local/bin/necropsy-spawner`, and shared by the environments.
3. For each environment: a spawner service (`necropsy-spawner`, and `necropsy-dev-spawner`), a starting lab allowlist sized to the VM's memory (a lower cpu limit on 2 vCPU or fewer; with both environments, two thirds of the labs go to production), an unprivileged deploy user, the blue-green deploy program ([docker.md](docker.md)), and the settings (`/etc/<name>/necropsy.env` and `deploy.env`: base URL, live domain, spawner, captcha, email, port, lab limit).
4. Adds a firewall rule (a small systemd unit, in Docker's `DOCKER-USER` chain, since Docker bypasses ufw) so only your proxy can reach the sites' ports.
5. Deploys the newest commit of each environment's branch, which must be pushed to the repository (`main` and `dev` by default). **A deploy tests the labs**: it builds the lab images, restarts the stack's spawner, and starts a real lab of every image, checking that it answers and that Docker has it with no network, a read-only root, no capabilities, no new privileges, an unprivileged user and hard limits. If anything is wrong it stops before moving traffic and puts the previous allowlist back.

It ends by printing what is left on your side: DNS, the wildcard certificates, the reverse proxy blocks, the Turnstile hostnames and the CI secrets. It is safe to run again, and that is how you update the shared spawner program and the firewall rule. The sites and their labs update through deploys, below.

Check it:

```bash
systemctl status necropsy-spawner
curl --unix-socket /run/necropsy/spawner.sock http://x/v1/instances   # [] when idle
docker ps --filter label=necropsy.range=1                              # the running labs
```

The spawner finds running containers again after a restart and keeps their expiry, so restarting it does not drop anyone's lab. Anything past its time, and any socket directory nothing owns, is removed at start and every ten seconds.

### How labs and the spawner ship

After installing, CI does the rest ([ci-cd.md](ci-cd.md)). A push to `main` (or `dev`) runs `necropsy-deploy` (or `necropsy-dev-deploy`) on the VM, which for a stack set up by the installer:

- builds that commit's lab images, tagged `necropsy-range/<stack>/<lab>:<commit>`, and writes the stack's allowlist (`/etc/<stack>/images.toml`) to point at them, with the stack's own `RANGE_MAX_INSTANCES`;
- restarts the stack's spawner, since it reads its allowlist once at start (running labs are found again and keep their time);
- tests every lab (`deploy/range/selftest.sh`), then moves traffic;
- keeps the last two deploys' lab images, for labs still running and a rollback, and removes older ones.

So adding a lab is a commit: `range/<name>/`, an entry in `range/images.toml`, and its pack. **The spawner program is different, because it runs as root:** only a *production* deploy rebuilds it (from `main`, reviewed through a pull request) and restarts the other stack's spawner too. A dev deploy never replaces it, so write access to the dev branch cannot become root on the VM. The settings that control this are in `deploy.env` (`RANGE_UNIT`, `RANGE_MAX_INSTANCES`, `RANGE_BUILD_SPAWNER`, `RANGE_CPUS_CAP`, `RANGE_SELFTEST`; see `deploy/bluegreen/deploy.env.example`).

CI also tests the range against real Docker on every push (`deploy/range/ci-test.sh`): it builds the labs, runs the real spawner, runs the self-test on every image and each lab's own test (`range/<lab>/test.sh`), and checks that an expired lab is reaped and a full range refuses another.

To do it by hand instead: `cargo build --release -p necropsy-spawner` and install the binary to `/usr/local/bin`; `sed 's/@NAME@/necropsy/g' deploy/range/necropsy-spawner.service.in > /etc/systemd/system/necropsy-spawner.service`; copy `range/images.toml` to `/etc/necropsy/images.toml` and run `range/build.sh`; set `NECROPSY_LIVE_DOMAIN`, `NECROPSY_SPAWNER_SOCKET=/run/necropsy/spawner.sock` and `NECROPSY_RANGE_DIR=/run/necropsy/range` in the site's environment file; and deploy. The blue-green compose file mounts `/run/<stack name>` from the host into the site's container at `/run/necropsy`. Without `NECROPSY_SPAWNER_SOCKET`, container puzzles are hidden everywhere, like live puzzles without a domain.

## Adding a lab

1. `range/<name>/` with a `Dockerfile` and whatever it needs. The contract with the spawner:
   - it **listens on a unix socket at `/run/sock/app.sock`** (a directory mounted from the host, writable by the image's user, so nothing is exposed on any network);
   - it runs as the user named in `images.toml` (`uid`);
   - it reads its flag from `$NECROPSY_FLAG` and puts it wherever the puzzle hides it (in the writable `/tmp`, since the root is read-only);
   - HTTP/1.1 over that socket. WebSockets and non-HTTP services are not supported.

   Most servers listen on TCP. The `socat` line at the end of `range/webshell-lab/entrypoint.sh` bridges loopback TCP to the socket, which is all it takes.
2. An entry in `range/images.toml` (this is the allowlist and where limits live), then `./range/build.sh`.
3. Optionally, `range/<name>/test.sh <spawner socket> <range dir> <image>`: a test of the lab's own behaviour, run by CI against the real spawner (see `range/webshell-lab/test.sh`). The generic self-test already checks that it answers and is locked down.
4. A pack with `modes = ["web", "live"]` and a `live.toml` containing:

   ```toml
   ttl_minutes = 45
   flag_prefix = "shell"
   [container]
   image = "webshell-lab"    # a key of images.toml
   ```

   A container scenario has no routes. The flag is `shell{...}`, different per instance, and the player submits it as usual. `cargo test` checks that every pack's image is on the allowlist.

Real samples are fine in a lab: it is their own throwaway box with no network. Keep what is in `files/` inert (a `.sample` suffix, say), as for any pack.

## How many players it holds

"Active users" here means **running instances**, which is how many players are mid-puzzle at once (a lab lives about as long as a person works on it, call it 15 to 30 minutes, then idles out). The site itself is tiny and is not the limit: it is a small Rust server, and one vCPU serves thousands of ordinary visitors a day. The labs are what cost something.

For the bundled `webshell-lab` (limits `memory_mb = 128`, `cpus = 0.5`, `pids = 64`):

- **Memory is the limit that matters.** An idle lab uses about 30 to 50 MB (a PHP server, `socat`, a shell now and then). A player can push theirs to the 128 MB cap on purpose, and the cap is hard (no swap), so it is what you plan against. Docker adds roughly 15 MB of overhead per container on top.
- **CPU** is mostly idle, a few percent of a core while someone is actively typing, with a short burst of about a CPU-second when a container starts. `cpus` is a ceiling, not a reservation.
- **Disk** is about 100 MB per image, shared by every instance; instances write only to RAM.
- **Reserve** about 1.5 GB of RAM and half a vCPU for the OS, Docker and the site.

| VM | Safe: every lab at its cap (143 MB) | Typical: ~60 MB each | Set `max_instances` to | Players per hour at ~30 min each |
| --- | --- | --- | --- | --- |
| 2 vCPU, 4 GB | 17 | 42 | **20** | about 40 |
| 4 vCPU, 8 GB | 46 | 110 | **50** | about 100 |
| 8 vCPU, 16 GB | 103 | 247 | **110** | about 220 |
| 16 vCPU, 32 GB | 218 | 520 | **230** | about 460 |

(RAM left after the reserve, divided by the per-lab figure.) "Safe" is the number that cannot run the machine out of memory whatever players do. "Typical" is what it holds when labs behave. The recommended value sits just above Safe: reaching it needs many players each deliberately filling their lab, which a captcha and a three-instance limit per account make hard, and if it happens the kernel kills the offending lab's processes, not the site. The installer sets it for you from the VM's memory (and splits it two thirds, one third when production and dev share the VM); to change it, set `RANGE_MAX_INSTANCES` in that stack's `/etc/<stack>/deploy.env` and deploy (a deploy rewrites the allowlist from it). Past it a new player sees "The range is full right now" and can try again.

CPU only becomes the limit on the smaller machines if many players run heavy commands at once; on a 2 vCPU box lower `cpus` to `0.25` so a handful of busy labs cannot starve the site. A heavier lab (a database, a Java app) changes the figures completely: measure it, not the table.

These are estimates from the image's footprint, not a load test of your VM. Measure on yours:

```bash
docker stats --no-stream --format 'table {{.Name}}\t{{.MemUsage}}\t{{.CPUPerc}}' $(docker ps -q --filter label=necropsy.range=1)
```

Start with the recommended number, watch real use for a week, and raise it if the memory you see is nearer "Typical" than "Safe".
