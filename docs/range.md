# The range: real servers for live puzzles

A [live puzzle](live.md) can be backed by a real server instead of simulated routes. Each player gets their own container, started just for them, running real software: a PHP site with a working webshell, a vulnerable app, whatever the puzzle needs. They talk to it at their own `https://<label>.necropsy.urashyl.dev` link, the same as any live puzzle.

Those containers run on a VM of their own, the **range**, shared by production and dev. The sites never run a lab themselves:

```text
                 ┌─ prod VM ─┐                       ┌──────────── range VM ─────────────┐
 player ─https─► │ necropsy  │ ──HTTP, token ──────► │ spawner ──unix socket──► lab      │
                 └───────────┘                       │    ▲  (no network)       lab      │
                 ┌─ dev VM ──┐                       │    │                     lab      │
 player ─https─► │ necropsy  │ ──HTTP, other token ► │    └─ docker                      │
                 └───────────┘                       └───────────────────────────────────┘
```

The range VM runs one program, the spawner. It serves the sites over the private network, one **tenant** each (`prod`, `dev`), and does three things for a tenant: it takes the lab images the site built, starts and stops labs from them, and passes the site's players' requests through to their lab.

## Security model

The point is to let players run things (a webshell executes their commands) without that reaching anything that matters. The layers:

- **Labs are on a VM with nothing else on it.** No site, no database, no accounts, no GitHub access. A lab that escapes its container lands on a machine that holds the spawner's tenant tokens and other players' labs, and nothing of either site.
- **Labs have no network.** They start with `--network=none`. They cannot reach the internet, the range VM's own services, the sites, or each other. The only way in is a unix socket, in a directory on the range VM, and the only program that connects to it is the spawner.
- **Locked down.** Read-only root, all capabilities dropped, `no-new-privileges`, an unprivileged user, a pid limit, hard memory and cpu limits (swap included), a small `noexec` tmpfs as the only writable place, no logs kept.
- **Tenants are separate.** Each site has its own token, its own images, its own allowlist and its own limits: a cap on how many labs, and on their memory together. One tenant cannot see, reach, extend or stop another's labs, cannot push an image under another's name, and cannot use up another's room. A site with a bad day (or a compromised one) can fill *its own* share and nothing else.
- **A site can only ask for what it pushed.** It can start a lab of an image on its own allowlist, with the limits on it (checked against fixed ranges, never `root`), and the only settings it can pass are the flag and the lab's id. No other image, mounts, network, privileged mode or environment.
- **Images are checked before they are loaded.** A site builds its labs and streams each as an image archive. Loading an archive applies the tags it carries, so the spawner reads the archive's own list of tags first, and refuses one that names anything outside `necropsy-range/<tenant>/`. The range VM never builds from the repository and never runs a program from it.
- **The network is private and filtered.** The spawner listens on the private network only, and a firewall on the range VM lets in only the site VMs. Every request carries the tenant's token (a random 256-bit value, of which the spawner keeps only a hash). Tokens travel in clear text on that network: that is acceptable on a private LAN you control, and is why the firewall matters. If the VMs ever share a network you do not control, put TLS in front (a reverse proxy on the range VM with client certificates) and use `https://` in `RANGE_URL`.
- **Disposable.** Every lab is a fresh container, removed with its socket directory when its time is up. Nothing a player does persists.

What this is **not** is a defence against a kernel or container-runtime escape on the range VM. Containers share its kernel. The VM boundary is what limits the damage, so keep Docker and the kernel patched there, and consider gVisor (`runsc`) or Kata as Docker's runtime for more: the spawner uses whatever default runtime Docker has.

A lab lives for its puzzle's `ttl_minutes`, and is **extended while it is used**: when less than half is left and a request arrives, it gets a full term again, up to 8 hours from when it started. An idle lab goes away on its own.

## Setting it up

Three VMs: the range, then each site. Debian or Ubuntu; the scripts install Docker where it is missing.

### 1. The range VM

As root, from a checkout of the repository:

```bash
sudo deploy/install-range.sh \
  --listen 192.168.100.50 \
  --tenant prod:192.168.100.21 \
  --tenant dev:192.168.100.22
```

`--listen` is this VM's address on the private network. A tenant is `NAME:ADDRESS`, the name of a site and the address of its VM. The first tenant gets twice the share of the others unless you give a weight (`dev:192.168.100.22:1`). Add `--dry-run` to see everything first.

It installs Docker, builds the spawner in a throwaway Rust container (as a static binary, so this VM needs no Rust), makes a **token for each tenant** and limits sized to the VM's memory, starts the spawner under systemd, and adds a firewall rule so only the tenants' addresses can reach its port. It is safe to run again: it updates the program and the limits and keeps the tokens.

It ends by telling you where each token is (`/etc/necropsy-range/tokens/<tenant>`, root only) and what to run on each site.

### 2. Each site VM

Copy a tenant's token to its VM as a root-only file, then on that VM:

```bash
sudo deploy/install-site.sh \
  --domain necropsy.urashyl.dev --proxy-ip 192.168.100.5 \
  --range-url http://192.168.100.50:9400 --range-tenant prod \
  --range-token-file /root/range-prod.token \
  --turnstile-site-key 0x4AAAAAAA... --turnstile-secret-file /root/turnstile-secret \
  --smtp-host smtp.example.com --smtp-user mailer --smtp-from "necropsy <no-reply@example.com>"
```

On the dev VM, the same with its own `--domain`, `--range-tenant dev`, its own token and `--branch dev`. The script checks that the range answers and accepts the token before it deploys. It does not build or run any lab: a deploy pushes them to the range. See [docker.md](docker.md) for what else it sets up, and `deploy/install-site.sh --help` for the rest.

### 3. CI

After that, pushes deploy ([ci-cd.md](ci-cd.md)).

### How labs ship

A deploy on a site VM, before it moves any traffic:

1. builds that commit's lab images, tagged `necropsy-range/<tenant>/<lab>:<image id>`, so a lab that did not change keeps its tag;
2. asks the range which images it has, and **pushes only the missing ones**;
3. sends the lab allowlist (`range/images.toml`, with each image rewritten to the one just built);
4. has the range **test every lab** (`POST /v1/selftest`): it starts each one, and checks that it answers on its socket and that Docker has it with no network, a read-only root, no capabilities, no new privileges, an unprivileged user and hard limits, and that, inside it, it has no interface but loopback, is not root and cannot write its root. If any lab fails, the range's previous allowlist is put back and the deploy stops with the site untouched;
5. keeps this deploy's lab images and the previous one's on the site VM, and removes older ones. The range removes a tenant's images that its current and previous allowlists no longer name.

The range applies a new allowlist at once, with no restart, and labs already running are not touched. So adding a lab is a commit: `range/<name>/`, its entry in `range/images.toml`, and its pack.

The spawner program itself is **not** updated by site deploys: it runs as root on the range VM, and a site, least of all the dev branch, should not be able to change what runs there. Update it by running `deploy/install-range.sh` again from an up-to-date checkout.

CI also tests the range against real Docker on every push (`deploy/range/ci-test.sh`): it starts the real spawner with two tenants, pushes the labs, runs the self-test and each lab's own test (`range/<lab>/test.sh`), and checks that tenants cannot touch each other, that an expired lab is reaped and that a full range refuses another.

Check a running range:

```bash
systemctl status necropsy-spawner
curl http://192.168.100.50:9400/healthz                      # ok (from a site VM)
docker ps --filter label=necropsy.range=1                    # the running labs, on the range VM
```

The spawner finds running labs again after a restart and keeps their expiry and their memory counted, so restarting it does not drop anyone's lab. Anything past its time, and any socket directory nothing owns, is removed at start and every ten seconds.

### When a site cannot reach the range

A start fails with "Could not start the instance", and the site's log says `the range refused <id>: 502 ... <cause>`. The cause is the end of that line (`Connection refused`, `timed out`, `rejects this site's token`). The same check runs when the site starts (look for `the range at ... answers` or `the range is not usable` in `docker logs necropsy-blue`), and the **Admin page shows it live** under Live puzzles: it is made from inside the container, which is where it matters, since a host that can reach the range is no proof that its container can.

| It says | It means |
| --- | --- |
| `Connection refused` | Nothing is listening on that address and port. On the range VM: `systemctl status necropsy-spawner`, and `ss -ltn \| grep 9400`. Did `install-range.sh` run, and is `--listen` this VM's address? |
| `timed out` | Something drops the traffic. On the range VM: `iptables -S NECROPSY-RANGE` must list this site VM's address (it is `--tenant NAME:ADDRESS`); and nothing in between may block the port. |
| `rejects this site's token` | The range is reached, but the token is not the one made for this tenant. Compare `/etc/necropsy/range.token` here with `/etc/necropsy-range/tokens/<tenant>` there. |
| works from the VM, not from the container | Docker's own networking. Try it the way the container does: `docker run --rm --network necropsy_internal curlimages/curl -sS -m 5 http://<range>:9400/healthz`. If that fails and `curl` on the VM works, check `sysctl net.ipv4.ip_forward` (must be 1), that the firewall's forward policy is not DROP (a ufw default of `DROP` for forwarding breaks Docker), and that a masquerade rule for the container network exists (`iptables -t nat -S POSTROUTING`). |

A different port that answers (a quick `python -m http.server`, say) shows the network between the VMs works, not that the spawner is up on 9400.

### The control API

For reference (the web server and the deploy program are its only clients). Every request carries `X-Necropsy-Token`; the token says which tenant it is.

| | |
| --- | --- |
| `GET /healthz` | `ok`, no token needed |
| `POST /v1/instances` | start a lab: `{"id", "image", "ttl_secs", "env": {"NECROPSY_FLAG", "NECROPSY_INSTANCE"}}` |
| `GET /v1/instances` | this tenant's labs |
| `POST /v1/instances/{id}/touch` | extend: `{"ttl_secs"}` |
| `DELETE /v1/instances/{id}` | stop |
| `GET /v1/images` | this tenant's images on the range, one per line |
| `POST /v1/images` | push an image (a `docker save` archive, streamed) |
| `GET`, `PUT /v1/allowlist` | read, replace this tenant's allowlist |
| `POST /v1/selftest` | test every lab on the allowlist; 200 with a report, or 500 naming what failed |
| any request with `X-Necropsy-Instance: <id>` | lab traffic: passed to that lab's socket |

## Adding a lab

1. `range/<name>/` with a `Dockerfile` and whatever it needs. The contract with the spawner:
   - it **listens on a unix socket at `/run/sock/app.sock`** (a directory the spawner mounts from the range VM, writable by the image's user, so nothing is exposed on any network);
   - it runs as the user named in `images.toml` (`uid`);
   - it reads its flag from `$NECROPSY_FLAG` and puts it wherever the puzzle hides it (in the writable `/tmp`, since the root is read-only);
   - HTTP/1.1 over that socket. WebSockets and non-HTTP services are not supported.

   Most servers listen on TCP. The `socat` line at the end of `range/webshell-lab/entrypoint.sh` bridges loopback TCP to the socket, which is all it takes.
2. An entry in `range/images.toml`: the limits the lab may use. It is a template; each deploy rewrites the image names.
3. Optionally, `range/<name>/test.sh <spawner url> <token> <lab>`: a test of the lab's own behaviour, run by CI against the real spawner (see `range/webshell-lab/test.sh`). The self-test already checks that it answers and is locked down.
4. A pack with `modes = ["web", "live"]` and a `live.toml` containing:

   ```toml
   ttl_minutes = 45
   flag_prefix = "shell"
   [container]
   image = "webshell-lab"    # a key of images.toml
   ```

   A container scenario has no routes. The flag is `shell{...}`, different per instance, and the player submits it as usual. `cargo test` checks that every pack's lab exists in `range/`.

Real samples are fine in a lab: it is a throwaway box with no network on a VM with nothing else on it. Keep what is in `files/` inert (a `.sample` suffix, say), as for any pack.

## How many players it holds

"Active users" here means **running labs**, which is how many players are mid-puzzle at once (a lab lives about as long as a person works on it, call it 15 to 30 minutes, then idles out). Only the range VM carries this load: the sites themselves are small Rust servers that a single vCPU and a gigabyte or two run comfortably.

For the bundled `webshell-lab` (limits `memory_mb = 128`, `cpus = 0.5`, `pids = 64`):

- **Memory is the limit that matters.** An idle lab uses about 30 to 50 MB (a PHP server, `socat`, a shell now and then). A player can push theirs to the 128 MB cap on purpose, and the cap is hard (no swap), so it is what you plan against. Docker adds roughly 15 MB of overhead per container on top.
- **CPU** is mostly idle, a few percent of a core while someone is actively typing, with a short burst of about a CPU-second when a lab starts. `cpus` is a ceiling, not a reservation.
- **Disk** is about 100 MB per image, per tenant; labs write only to RAM.
- **Reserve** about 1 GB of RAM for the OS, Docker and the spawner. The range VM has nothing else on it.

| Range VM | Safe: every lab at its cap (143 MB) | Typical: ~60 MB each | Labs for prod / dev (two thirds, one third) | Players per hour at ~30 min each |
| --- | --- | --- | --- | --- |
| 2 vCPU, 4 GB | 21 | 51 | **15 / 7** | about 44 |
| 4 vCPU, 8 GB | 50 | 119 | **36 / 18** | about 108 |
| 8 vCPU, 16 GB | 107 | 256 | **78 / 39** | about 234 |
| 16 vCPU, 32 GB | 221 | 529 | **162 / 81** | about 486 |

(RAM left after the reserve, divided by the per-lab figure; the limits are what `install-range.sh` writes.) "Safe" is the number that cannot run the machine out of memory whatever players do. "Typical" is what it holds when labs behave. The limits written sit a little above Safe: reaching them needs many players each deliberately filling their lab, which a captcha and a three-lab limit per account make hard, and if it happens the kernel kills the offending lab's processes, not the spawner. Each tenant's **memory budget** is its lab limit times the number of labs, so a tenant can never take more than its share: dev being busy does not take room from prod.

Past a tenant's limit a new player sees "The range is full right now" and can try again. To change the split or the limits, edit `/etc/necropsy-range/tenants.toml` (or run `install-range.sh` again with different weights) and restart the spawner; running labs are kept.

CPU only becomes the limit on the smaller machines, if many players run heavy commands at once; on a 2 vCPU range lower each lab's `cpus` in `range/images.toml` to `0.25`. A heavier lab (a database, a Java app) changes the figures completely: measure it, not the table.

These are estimates from the image's footprint, not a load test of your VM. Measure on yours:

```bash
docker stats --no-stream --format 'table {{.Name}}\t{{.MemUsage}}\t{{.CPUPerc}}' $(docker ps -q --filter label=necropsy.range=1)
```

Start with the written limits, watch real use for a week, and raise them if the memory you see is nearer "Typical" than "Safe".
