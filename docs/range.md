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
  --turnstile-site-key 0x4AAAAAAA... --turnstile-secret-file /root/turnstile-secret \
  --smtp-host smtp.example.com --smtp-user mailer --smtp-from "necropsy <no-reply@example.com>"
```

`--proxy-ip` is the address your reverse proxy connects from. Add `--dry-run` first to see everything it would do. Secrets are never options (they would land in `ps` and your shell history): the Turnstile secret goes in a file, and the SMTP password in `NECROPSY_SMTP_PASSWORD`. Run `deploy/install-vm.sh --help` for the rest.

What it does, in order:

1. Installs what is missing: Docker from Docker's own apt repository, plus git, curl, iptables and friends.
2. Builds the spawner inside a throwaway Rust container, as a static binary, so the VM needs no Rust and the result does not depend on its libc.
3. Installs `/etc/necropsy/images.toml` with `max_instances` sized to the VM's memory (and a lower cpu limit on 2 vCPU or fewer), and builds the lab images. A later run keeps the `max_instances` you have.
4. Installs and starts the spawner under systemd.
5. **Tests a real lab**: starts `webshell-lab` through the spawner, and checks that it answers on its socket, holds its flag, runs as an unprivileged user on a read-only root, and cannot reach the internet. If any of that is wrong it stops before touching the site.
6. Creates an unprivileged `deploy` user and installs the blue-green deploy program ([docker.md](docker.md)), which CI deploys through.
7. Writes the settings (`/etc/necropsy/necropsy.env`: base URL, live domain, spawner, captcha, email) and tells the site to publish its port to your proxy only.
8. Adds a firewall rule (a small systemd unit, in Docker's `DOCKER-USER` chain, since Docker bypasses ufw) so only your proxy can reach the site's port.
9. Deploys the newest commit of the branch, which must be pushed to the repository (`--branch`, `main` by default).

It ends by printing what is left on your side: DNS, the wildcard certificate, the reverse proxy block and the Turnstile hostname. It is safe to run again, and that is also how you update the spawner and lab images (the site itself updates through `necropsy-deploy`, or CI).

Check it:

```bash
systemctl status necropsy-spawner
curl --unix-socket /run/necropsy/spawner.sock http://x/v1/instances   # [] when idle
docker ps --filter label=necropsy.range=1                              # the running labs
```

To do it by hand instead, the script's steps are plain commands: `cargo build --release -p necropsy-spawner` and install the binary to `/usr/local/bin`, install `range/images.toml` to `/etc/necropsy/images.toml`, run `range/build.sh`, install `deploy/range/necropsy-spawner.service`, set `NECROPSY_LIVE_DOMAIN`, `NECROPSY_SPAWNER_SOCKET=/run/necropsy/spawner.sock` and `NECROPSY_RANGE_DIR=/run/necropsy/range` in the site's environment file, and deploy. The blue-green compose file already mounts `/run/necropsy` into the site's container (override the host side with `NECROPSY_RANGE_HOST_DIR`). Without `NECROPSY_SPAWNER_SOCKET`, container puzzles are hidden everywhere, like live puzzles without a domain.

The spawner finds running containers again after a restart and keeps their expiry, so restarting it does not drop anyone's lab. Anything past its time, and any socket directory nothing owns, is removed at start and every ten seconds.

## Adding a lab

1. `range/<name>/` with a `Dockerfile` and whatever it needs. The contract with the spawner:
   - it **listens on a unix socket at `/run/sock/app.sock`** (a directory mounted from the host, writable by the image's user, so nothing is exposed on any network);
   - it runs as the user named in `images.toml` (`uid`);
   - it reads its flag from `$NECROPSY_FLAG` and puts it wherever the puzzle hides it (in the writable `/tmp`, since the root is read-only);
   - HTTP/1.1 over that socket. WebSockets and non-HTTP services are not supported.

   Most servers listen on TCP. The `socat` line at the end of `range/webshell-lab/entrypoint.sh` bridges loopback TCP to the socket, which is all it takes.
2. An entry in `range/images.toml` (this is the allowlist and where limits live), then `./range/build.sh`.
3. A pack with `modes = ["web", "live"]` and a `live.toml` containing:

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

(RAM left after the reserve, divided by the per-lab figure.) "Safe" is the number that cannot run the machine out of memory whatever players do. "Typical" is what it holds when labs behave. The recommended value sits just above Safe: reaching it needs many players each deliberately filling their lab, which a captcha and a three-instance limit per account make hard, and if it happens the kernel kills the offending lab's processes, not the site. Put the number in `max_instances` (in `images.toml`, or `SPAWNER_MAX_INSTANCES`). Past it a new player sees "The range is full right now" and can try again.

CPU only becomes the limit on the smaller machines if many players run heavy commands at once; on a 2 vCPU box lower `cpus` to `0.25` so a handful of busy labs cannot starve the site. A heavier lab (a database, a Java app) changes the figures completely: measure it, not the table.

These are estimates from the image's footprint, not a load test of your VM. Measure on yours:

```bash
docker stats --no-stream --format 'table {{.Name}}\t{{.MemUsage}}\t{{.CPUPerc}}' $(docker ps -q --filter label=necropsy.range=1)
```

Start with the recommended number, watch real use for a week, and raise it if the memory you see is nearer "Typical" than "Safe".
