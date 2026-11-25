#!/usr/bin/env bash
# Installs the whole of necropsy on a dedicated VM: the site (blue-green, in
# Docker), the live-puzzle range (spawner and lab images), the settings, a
# firewall rule for the site's port, and a test that a real lab starts, is
# locked down and can be reached. Run as root, from a checkout of the
# repository, on the VM:
#
#   sudo deploy/install-vm.sh --domain necropsy.urashyl.dev --proxy-ip 192.168.100.5 \
#        --turnstile-site-key 0x4AAA... --smtp-host smtp.example.com ...
#
# Secrets are never taken as options (they would show up in `ps` and in your
# shell history): pass them in the environment, or by file.
#   NECROPSY_TURNSTILE_SECRET   or  --turnstile-secret-file FILE
#   NECROPSY_SMTP_PASSWORD
#
# It is safe to run again: it updates what changed, keeps your settings, and
# redeploys. Use --dry-run to see everything it would do first.
#
# What it does, in order:
#   1. installs what is missing (Docker from Docker's own apt repository on
#      Debian or Ubuntu; git, curl, iptables and friends)
#   2. builds the spawner (inside a throwaway Rust container, so the VM needs
#      no Rust, and as a static binary, so it runs on any distribution)
#   3. installs the image allowlist, sized to this VM, and builds the lab images
#   4. installs and starts the spawner under systemd
#   5. installs the blue-green deploy program (deploy/bluegreen/install-system.sh)
#      with an unprivileged deploy user
#   6. writes the settings: base URL, live domain, spawner, captcha, email
#   7. restricts the site's port to your reverse proxy
#   8. deploys the newest commit of the branch
#
# Between 4 and 5 it starts a real lab through the spawner and checks that it
# works and is isolated, so a broken range is found before the site is touched.
#
# Options:
#   --domain NAME          the site's public name, e.g. necropsy.urashyl.dev
#                          (required). Live puzzles are served at *.NAME
#   --proxy-ip ADDR        the address your reverse proxy connects from. Only it
#                          may reach the site's port, and only it is believed
#                          about visitors' addresses (required, unless
#                          --proxy-on-host)
#   --proxy-on-host        the reverse proxy runs on this VM, on loopback
#   --repo URL             https URL to build releases from (default: this
#                          checkout's origin). It must be reachable without a login
#   --branch NAME          the only branch that is built (main)
#   --deploy-user NAME     the unprivileged account that may run deploys through
#                          sudo, for CI (deploy). Created if it does not exist
#   --turnstile-site-key K Cloudflare Turnstile site key (public)
#   --turnstile-secret-file FILE
#                          file holding the Turnstile secret key
#   --smtp-host H  --smtp-port P  --smtp-user U  --smtp-from "name <addr>"
#                          outgoing email (accounts need it). TLS is STARTTLS on
#                          587 unless you edit /etc/necropsy/necropsy.env
#   --max-instances N      most labs running at once (default: sized to this
#                          VM's memory, see docs/range.md)
#   --no-deploy            do everything except deploying the site
#   --no-selftest          skip starting a test lab
#   --no-firewall          do not touch the firewall
#   --dry-run              print what would happen and change nothing
#   -h, --help
#
# Layout it creates:
#   /usr/local/bin/necropsy-spawner         the spawner (static binary)
#   /etc/necropsy/images.toml               the lab allowlist and limits
#   /etc/necropsy/necropsy.env              the site's settings (mode 600)
#   /etc/necropsy/deploy.env                deploy options
#   /etc/systemd/system/necropsy-spawner.service
#   /etc/systemd/system/necropsy-firewall.service
#   /usr/local/bin/necropsy-deploy          the blue-green deploy program
#   /run/necropsy/                          spawner socket and lab sockets (tmpfs)

set -euo pipefail
shopt -s inherit_errexit

die() { echo "error: $*" >&2; exit 1; }
info() { echo "==> $*" >&2; }
warn() { echo "warning: $*" >&2; }
usage() { sed -n '2,200p' "${BASH_SOURCE[0]}" | sed -n '1,/^set -euo/p' | sed '$d' | sed 's/^# \{0,1\}//'; }

# ---- settings ----------------------------------------------------------------

domain="" proxy_ip="" proxy_on_host=0 repo="" branch=main deploy_user=deploy
site_key="" secret_file="" smtp_host="" smtp_port="" smtp_user="" smtp_from=""
max_instances="" do_deploy=1 do_selftest=1 do_firewall=1 dry=0

CONF_DIR=/etc/necropsy
BIN_DIR=/usr/local/bin
LIB_DIR=/usr/local/lib/necropsy
RANGE_SOCKET=/run/necropsy/spawner.sock
RANGE_DIR=/run/necropsy/range
SYSTEMD_DIR=/etc/systemd/system

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$here/.." && pwd)"

# Everything that changes the machine goes through run or write_file, so that
# --dry-run can show it instead of doing it.
run() {
  if [ "$dry" = 1 ]; then printf '+ %s\n' "$*" >&2; else "$@"; fi
}
write_file() { # path mode  (content on stdin)
  if [ "$dry" = 1 ]; then
    printf '+ write %s (mode %s)\n' "$1" "$2" >&2
    cat > /dev/null
  else
    local tmp
    tmp="$(mktemp "$1.XXXXXX")"
    cat > "$tmp"
    chmod "$2" "$tmp"
    mv -f "$tmp" "$1"
  fi
}

# ---- pure helpers (tested by deploy/test/install-vm.sh) ------------------------

# How many labs this much memory holds, planning at 143 MB each (the 128 MB cap
# plus Docker's overhead) after a 1.5 GB reserve for the OS, Docker and the
# site, and allowing a little over that since few labs reach their cap. This is
# the "Set max_instances to" column of docs/range.md.
suggest_max_instances() { # total memory in MB
  local avail=$(( $1 - 1536 ))
  [ "$avail" -gt 0 ] || { echo 2; return; }
  local n=$(( avail * 11 / 10 / 143 ))
  [ "$n" -ge 2 ] || n=2
  echo "$n"
}

# Sets KEY=VALUE in an env file: replaces the line if there is one, uncomments a
# commented example if there is one, and appends otherwise. The value is
# checked, since the file is read by Docker Compose and by shells.
set_env() { # file key value
  local file="$1" key="$2" value="$3"
  case "$value" in
    *$'\n'*|*\'*|*'$'*|*'\'*|*'`'*) die "the value for $key contains a character that is not allowed in the settings file (newline, quote, \$, backslash or backtick)" ;;
  esac
  case "$value" in
    *[[:space:]]*|*'<'*|*'>'*) value="\"$value\"" ;;
  esac
  local tmp
  tmp="$(mktemp "$file.XXXXXX")"
  awk -v k="$key" -v v="$value" '
    BEGIN { done = 0 }
    $0 ~ "^" k "=" { if (!done) { print k "=" v; done = 1 }; next }
    $0 ~ "^#" k "=" && !done { print k "=" v; done = 1; next }
    { print }
    END { if (!done) print k "=" v }
  ' "$file" > "$tmp"
  cat "$tmp" > "$file"
  rm -f "$tmp"
}

# Lowers (never raises) every `cpus = N` in an images file to at most $2.
cap_cpus() { # file max
  local tmp
  tmp="$(mktemp "$1.XXXXXX")"
  awk -v c="$2" '/^cpus *=/ { if ($3 + 0 > c + 0) $3 = c } { print }' "$1" > "$tmp"
  cat "$tmp" > "$1"
  rm -f "$tmp"
}

valid_domain() { [[ "$1" =~ ^([a-z0-9]([a-z0-9-]*[a-z0-9])?\.)+[a-z]{2,}$ ]]; }
valid_addr() { [[ "$1" =~ ^[0-9a-fA-F:.]+$ ]] && [[ "$1" == *.* || "$1" == *:* ]]; }

# ---- arguments -----------------------------------------------------------------

parse_args() {
  while [ $# -gt 0 ]; do
    case "$1" in
      --domain) domain="${2:?--domain needs a name}"; shift 2 ;;
      --proxy-ip) proxy_ip="${2:?--proxy-ip needs an address}"; shift 2 ;;
      --proxy-on-host) proxy_on_host=1; shift ;;
      --repo) repo="${2:?--repo needs a URL}"; shift 2 ;;
      --branch) branch="${2:?--branch needs a name}"; shift 2 ;;
      --deploy-user) deploy_user="${2:?--deploy-user needs a name}"; shift 2 ;;
      --turnstile-site-key) site_key="${2:?}"; shift 2 ;;
      --turnstile-secret-file) secret_file="${2:?}"; shift 2 ;;
      --smtp-host) smtp_host="${2:?}"; shift 2 ;;
      --smtp-port) smtp_port="${2:?}"; shift 2 ;;
      --smtp-user) smtp_user="${2:?}"; shift 2 ;;
      --smtp-from) smtp_from="${2:?}"; shift 2 ;;
      --max-instances) max_instances="${2:?}"; shift 2 ;;
      --no-deploy) do_deploy=0; shift ;;
      --no-selftest) do_selftest=0; shift ;;
      --no-firewall) do_firewall=0; shift ;;
      --dry-run) dry=1; shift ;;
      -h|--help) usage; exit 0 ;;
      *) usage >&2; die "unknown option: $1" ;;
    esac
  done
}

validate_args() {
  [ -n "$domain" ] || die "--domain is required"
  valid_domain "$domain" || die "--domain must be a plain lower-case domain name like necropsy.urashyl.dev"
  if [ "$proxy_on_host" = 1 ]; then
    [ -z "$proxy_ip" ] || die "use either --proxy-ip or --proxy-on-host, not both"
  else
    [ -n "$proxy_ip" ] || die "--proxy-ip is required (your reverse proxy's address), or --proxy-on-host"
    valid_addr "$proxy_ip" || die "--proxy-ip must be a single IP address, like 192.168.100.5"
  fi
  [[ "$branch" =~ ^[A-Za-z0-9][A-Za-z0-9._/-]{0,99}$ ]] && [[ "$branch" != *..* ]] \
    || die "--branch must be a plain branch name"
  [[ "$deploy_user" =~ ^[a-z_][a-z0-9_-]*$ ]] || die "--deploy-user is not a valid user name"
  [ "$deploy_user" != root ] || die "the deploy user must not be root"
  if [ -n "$max_instances" ]; then
    [[ "$max_instances" =~ ^[0-9]+$ ]] && [ "$max_instances" -ge 1 ] || die "--max-instances must be a number"
  fi
  [ -z "$smtp_port" ] || [[ "$smtp_port" =~ ^[0-9]+$ ]] || die "--smtp-port must be a number"
  [ -z "$secret_file" ] || [ -f "$secret_file" ] || die "$secret_file does not exist"
  if [ -n "$site_key" ] && [ -z "${NECROPSY_TURNSTILE_SECRET:-}" ] && [ -z "$secret_file" ]; then
    die "a Turnstile site key needs its secret: set NECROPSY_TURNSTILE_SECRET or pass --turnstile-secret-file"
  fi
  if [ -z "$site_key" ] && { [ -n "${NECROPSY_TURNSTILE_SECRET:-}" ] || [ -n "$secret_file" ]; }; then
    die "a Turnstile secret needs its site key (--turnstile-site-key)"
  fi
  if [ -n "$repo" ]; then
    [[ "$repo" =~ ^https://[A-Za-z0-9._/-]+$ ]] || die "--repo must be an https URL like https://github.com/owner/name"
  fi
}

# ---- steps ---------------------------------------------------------------------

preflight() {
  if [ "$dry" = 0 ]; then
    [ "$(id -u)" -eq 0 ] || die "run this as root (sudo)"
    [ "$(uname -s)" = Linux ] || die "this installs onto Linux"
  fi
  [ -f "$repo_root/range/images.toml" ] && [ -f "$repo_root/deploy/bluegreen/install-system.sh" ] \
    || die "run this from a checkout of the necropsy repository (deploy/install-vm.sh)"

  if [ -z "$repo" ]; then
    local origin
    origin="$(git -C "$repo_root" remote get-url origin 2>/dev/null || true)"
    origin="${origin%.git}"
    case "$origin" in
      git@github.com:*) origin="https://github.com/${origin#git@github.com:}" ;;
    esac
    [[ "$origin" =~ ^https://[A-Za-z0-9._/-]+$ ]] \
      || die "cannot work out an https repository URL from this checkout's origin ($origin): pass --repo"
    repo="$origin"
  fi
  info "site https://$domain, live puzzles at *.$domain, building $repo ($branch)"

  total_mb=$(( $(awk '/^MemTotal:/ {print $2}' /proc/meminfo) / 1024 ))
  cpus_n="$(nproc)"
  info "this VM: $cpus_n vCPU, ${total_mb} MB"
  [ "$total_mb" -ge 2000 ] || warn "under 2 GB of memory: the site will run, but there is little room for labs"
}

apt_available() { command -v apt-get >/dev/null 2>&1; }

install_packages() {
  local need=()
  local tool pkg
  for pair in git:git curl:curl flock:util-linux ip:iproute2 iptables:iptables awk:gawk; do
    tool="${pair%%:*}" pkg="${pair##*:}"
    command -v "$tool" >/dev/null 2>&1 || need+=("$pkg")
  done
  command -v sudo >/dev/null 2>&1 || need+=(sudo)
  command -v visudo >/dev/null 2>&1 || need+=(sudo)
  if [ ${#need[@]} -gt 0 ]; then
    info "installing packages: ${need[*]}"
    apt_available || die "missing ${need[*]}, and this is not a Debian or Ubuntu system: install them and run again"
    run env DEBIAN_FRONTEND=noninteractive apt-get update -qq
    run env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq "${need[@]}"
  fi

  if ! command -v docker >/dev/null 2>&1 || ! docker compose version >/dev/null 2>&1; then
    install_docker
  fi
  run systemctl enable --now docker
  if [ "$dry" = 0 ]; then
    docker info >/dev/null 2>&1 || die "cannot talk to the Docker daemon"
  fi
}

# Docker's own apt repository, following https://docs.docker.com/engine/install/
install_docker() {
  info "installing Docker"
  apt_available || die "Docker is not installed, and this is not Debian or Ubuntu. Install Docker Engine with the compose plugin (https://docs.docker.com/engine/install/) and run this again"
  local id codename
  # shellcheck disable=SC1091
  id="$(. /etc/os-release && echo "$ID")"
  # shellcheck disable=SC1091
  codename="$(. /etc/os-release && echo "${UBUNTU_CODENAME:-$VERSION_CODENAME}")"
  case "$id" in debian|ubuntu) ;; *) die "automatic Docker install supports Debian and Ubuntu, not $id. Install Docker Engine with the compose plugin and run this again" ;; esac
  run env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq ca-certificates curl
  run install -m 0755 -d /etc/apt/keyrings
  run curl -fsSL "https://download.docker.com/linux/$id/gpg" -o /etc/apt/keyrings/docker.asc
  run chmod a+r /etc/apt/keyrings/docker.asc
  local arch
  arch="$(dpkg --print-architecture 2>/dev/null || echo amd64)"
  write_file /etc/apt/sources.list.d/docker.list 644 <<EOF
deb [arch=$arch signed-by=/etc/apt/keyrings/docker.asc] https://download.docker.com/linux/$id $codename stable
EOF
  run env DEBIAN_FRONTEND=noninteractive apt-get update -qq
  run env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
    docker-ce docker-ce-cli containerd.io docker-compose-plugin
}

# The spawner is built in a Rust container, as a static (musl) binary: the VM
# needs no Rust toolchain, and the result does not depend on its libc.
build_spawner() {
  info "building the spawner"
  local rust_version out
  rust_version="$(sed -n 's/^ARG RUST_VERSION=//p' "$repo_root/Dockerfile" | head -n 1)"
  [ -n "$rust_version" ] || die "cannot find the Rust version in the Dockerfile"
  out="$(mktemp -d)"
  run docker run --rm \
    -v "$repo_root":/src:ro \
    -v necropsy-build-cargo:/usr/local/cargo/registry \
    -v necropsy-build-target:/target \
    -v "$out":/out \
    -e CARGO_TARGET_DIR=/target \
    -w /src \
    "rust:${rust_version}-alpine" \
    sh -c 'apk add --no-cache musl-dev >/dev/null && cargo build --release --locked -p necropsy-spawner && cp /target/release/necropsy-spawner /out/'
  run install -d -m 755 "$BIN_DIR"
  if [ "$dry" = 0 ]; then
    [ -x "$out/necropsy-spawner" ] || die "the spawner did not build"
    install -m 755 "$out/necropsy-spawner" "$BIN_DIR/necropsy-spawner.new"
    mv -f "$BIN_DIR/necropsy-spawner.new" "$BIN_DIR/necropsy-spawner"
    rm -rf "$out"
  else
    echo "+ install the built binary as $BIN_DIR/necropsy-spawner" >&2
  fi
}

install_images() {
  info "installing the lab allowlist and building the lab images"
  run install -d -m 755 "$CONF_DIR"
  local src="$repo_root/range/images.toml" dst="$CONF_DIR/images.toml" tmp max cpu_cap
  tmp="$(mktemp)"
  cp "$src" "$tmp"

  # Keep an earlier choice of max_instances unless one is given now.
  max="$max_instances"
  if [ -z "$max" ] && [ -f "$dst" ]; then
    max="$(sed -n 's/^max_instances *= *\([0-9][0-9]*\).*/\1/p' "$dst" | head -n 1)"
  fi
  [ -n "$max" ] || max="$(suggest_max_instances "$total_mb")"
  sed -i "s/^max_instances *=.*/max_instances = $max/" "$tmp"
  info "at most $max labs at once"

  # On a small VM a few busy labs must not starve the site.
  if [ "$cpus_n" -le 2 ]; then
    cpu_cap=0.25
    cap_cpus "$tmp" "$cpu_cap"
    info "$cpus_n vCPU: labs limited to $cpu_cap of a CPU each"
  fi
  if [ "$dry" = 1 ]; then
    echo "+ write $dst" >&2
  else
    install -m 644 "$tmp" "$dst"
  fi
  rm -f "$tmp"

  run "$repo_root/range/build.sh"
}

install_spawner_service() {
  info "installing the spawner service"
  if [ "$dry" = 0 ]; then
    install -m 644 "$repo_root/deploy/range/necropsy-spawner.service" "$SYSTEMD_DIR/necropsy-spawner.service"
  else
    echo "+ install necropsy-spawner.service" >&2
  fi
  run systemctl daemon-reload
  run systemctl enable necropsy-spawner
  run systemctl restart necropsy-spawner
  if [ "$dry" = 0 ]; then
    local i
    for i in $(seq 1 50); do
      [ -S "$RANGE_SOCKET" ] && break
      sleep 0.2
    done
    [ -S "$RANGE_SOCKET" ] || die "the spawner did not start. See: journalctl -u necropsy-spawner -n 50"
    curl -fsS --unix-socket "$RANGE_SOCKET" http://spawner/healthz >/dev/null \
      || die "the spawner does not answer. See: journalctl -u necropsy-spawner -n 50"
  fi
}

ensure_deploy_user() {
  if id -u "$deploy_user" >/dev/null 2>&1; then
    info "deploy user $deploy_user exists"
    return
  fi
  info "creating the deploy user $deploy_user (no password; add your CI's public key to ~$deploy_user/.ssh/authorized_keys)"
  run useradd --create-home --shell /bin/bash "$deploy_user"
}

install_deploy_program() {
  info "installing the blue-green deploy program"
  run "$repo_root/deploy/bluegreen/install-system.sh" --user "$deploy_user" --repo "$repo" --branch "$branch"
}

write_settings() {
  info "writing the settings"
  if [ "$dry" = 1 ]; then
    echo "+ set in $CONF_DIR/necropsy.env: NECROPSY_BASE_URL NECROPSY_LIVE_DOMAIN NECROPSY_SPAWNER_SOCKET NECROPSY_RANGE_DIR (and captcha and email if given)" >&2
    echo "+ set in $CONF_DIR/deploy.env: NECROPSY_BIND NECROPSY_EDGE_TRUSTED" >&2
    return
  fi
  local env="$CONF_DIR/necropsy.env" denv="$CONF_DIR/deploy.env"
  [ -f "$env" ] || die "$env was not created by the deploy installer"
  set_env "$env" NECROPSY_BASE_URL "https://$domain"
  set_env "$env" NECROPSY_LIVE_DOMAIN "$domain"
  set_env "$env" NECROPSY_SPAWNER_SOCKET "$RANGE_SOCKET"
  set_env "$env" NECROPSY_RANGE_DIR "$RANGE_DIR"

  if [ -n "$site_key" ]; then
    local secret="${NECROPSY_TURNSTILE_SECRET:-}"
    [ -z "$secret_file" ] || secret="$(tr -d '[:space:]' < "$secret_file")"
    set_env "$env" NECROPSY_TURNSTILE_SITE_KEY "$site_key"
    set_env "$env" NECROPSY_TURNSTILE_SECRET "$secret"
  else
    warn "no Turnstile keys: anyone (or any script) can create accounts and start labs. Add them to $env and redeploy"
  fi

  if [ -n "$smtp_host" ]; then
    set_env "$env" NECROPSY_SMTP_HOST "$smtp_host"
    set_env "$env" NECROPSY_SMTP_PORT "${smtp_port:-587}"
    [ -z "$smtp_user" ] || set_env "$env" NECROPSY_SMTP_USERNAME "$smtp_user"
    [ -z "${NECROPSY_SMTP_PASSWORD:-}" ] || set_env "$env" NECROPSY_SMTP_PASSWORD "$NECROPSY_SMTP_PASSWORD"
    [ -z "$smtp_from" ] || set_env "$env" NECROPSY_SMTP_FROM "$smtp_from"
  else
    warn "no email settings: registration and password reset stay closed until NECROPSY_SMTP_* are set in $env"
  fi
  chmod 600 "$env"

  # The reverse proxy is another machine (or this one, on loopback).
  if [ "$proxy_on_host" = 1 ]; then
    set_env "$denv" NECROPSY_BIND 127.0.0.1
  else
    set_env "$denv" NECROPSY_BIND 0.0.0.0
    set_env "$denv" NECROPSY_EDGE_TRUSTED "$proxy_ip"
  fi
  chmod 644 "$denv"
}

# Docker's published ports bypass ufw and firewalld, so the site's port is
# closed to everything but your proxy in Docker's own DOCKER-USER chain, by a
# small unit that runs after Docker starts.
install_firewall() {
  if [ "$do_firewall" = 0 ] || [ "$proxy_on_host" = 1 ]; then
    return
  fi
  if [[ "$proxy_ip" == *:* ]]; then
    warn "an IPv6 proxy address: not adding a firewall rule. Restrict port 8080 to it yourself"
    return
  fi
  info "restricting the site's port to $proxy_ip"
  run install -d -m 755 "$LIB_DIR"
  write_file "$LIB_DIR/firewall.sh" 755 <<'EOF'
#!/bin/sh
# Only the reverse proxy may reach the site's published port. In the DOCKER-USER
# chain the port is the container's own, always 8080, whatever it is published
# as. Scoped to the external interface so the containers still talk to each other.
set -eu
proxy="$1"
iface="$(ip -o route get 1.1.1.1 2>/dev/null | sed -n 's/.* dev \([^ ]*\).*/\1/p')"
[ -n "$iface" ] || { echo "no default route: cannot work out the external interface" >&2; exit 1; }
while iptables -D DOCKER-USER -i "$iface" -p tcp --dport 8080 ! -s "$proxy" -j DROP 2>/dev/null; do :; done
iptables -I DOCKER-USER -i "$iface" -p tcp --dport 8080 ! -s "$proxy" -j DROP
EOF
  write_file "$SYSTEMD_DIR/necropsy-firewall.service" 644 <<EOF
[Unit]
Description=necropsy: only the reverse proxy may reach the site
After=docker.service
Requires=docker.service
PartOf=docker.service

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=$LIB_DIR/firewall.sh $proxy_ip

[Install]
WantedBy=multi-user.target docker.service
EOF
  run systemctl daemon-reload
  run systemctl enable necropsy-firewall
  run systemctl restart necropsy-firewall
}

deploy_site() {
  [ "$do_deploy" = 1 ] || { info "skipping the deploy (--no-deploy). Later: sudo necropsy-deploy deploy <commit on $branch>"; return; }
  info "deploying the newest commit of $branch"
  local sha
  if [ "$dry" = 1 ]; then
    echo "+ necropsy-deploy deploy <newest commit of $branch at $repo>" >&2
    return
  fi
  sha="$(git ls-remote "$repo" "refs/heads/$branch" | awk '{print $1}' | head -n 1)"
  [[ "$sha" =~ ^[0-9a-f]{40}$ ]] \
    || die "branch $branch was not found at $repo. Push it there first, or pick another with --branch"
  "$BIN_DIR/necropsy-deploy" deploy "$sha"
}

# A real lab, started the way the site will start it: it must come up, answer,
# run as an unprivileged user on a read-only root, and have no network.
selftest() {
  [ "$do_selftest" = 1 ] || return 0
  info "testing a real lab"
  if [ "$dry" = 1 ]; then
    echo "+ start the webshell-lab through the spawner, check it answers, is read-only, unprivileged and offline, then stop it" >&2
    return
  fi
  local id=selftest0001 flag='selftest{ok}' dir="$RANGE_DIR/selftest0001" sock out
  sock="$dir/app.sock"
  local api=(curl -fsS --unix-socket "$RANGE_SOCKET")
  "${api[@]}" -X DELETE "http://spawner/v1/instances/$id" >/dev/null 2>&1 || true

  "${api[@]}" -X POST http://spawner/v1/instances -H 'content-type: application/json' \
    -d "{\"id\":\"$id\",\"image\":\"webshell-lab\",\"ttl_secs\":120,\"env\":{\"NECROPSY_FLAG\":\"$flag\"}}" >/dev/null \
    || die "the spawner could not start the test lab. See: journalctl -u necropsy-spawner -n 50"

  local lab=(curl -fsS --max-time 10 --unix-socket "$sock")
  out="$("${lab[@]}" http://lab/)" || die "the test lab does not answer on its socket"
  grep -q "Greenleaf" <<<"$out" || die "the test lab answered, but not with its site"

  local shell=("${lab[@]}" -H 'X-Auth: 4c1f9ab2' http://lab/wp-content/uploads/cache/.thumbs.php)
  out="$("${shell[@]}" --data-urlencode 'q=cat /tmp/.stage/archive-0412.txt')"
  grep -q "$flag" <<<"$out" || die "the lab's flag is not where the puzzle expects it"

  out="$("${shell[@]}" --data-urlencode 'q=id -u')"
  [ "$(tr -d '[:space:]' <<<"$out")" = 10001 ] || die "the lab is not running as an unprivileged user (uid: $out)"

  out="$("${shell[@]}" --data-urlencode 'q=touch /usr/x 2>&1; echo rc=$?')"
  grep -q 'rc=[1-9]' <<<"$out" || die "the lab's filesystem is writable"

  out="$("${shell[@]}" --data-urlencode 'q=wget -T 3 -q -O- http://1.1.1.1/ >/dev/null 2>&1 && echo ONLINE || echo OFFLINE')"
  grep -q OFFLINE <<<"$out" || die "the lab can reach the internet"

  "${api[@]}" -X DELETE "http://spawner/v1/instances/$id" >/dev/null
  [ ! -e "$dir" ] || die "the test lab's directory was not removed"
  docker ps -a --format '{{.Names}}' | grep -qx "nr-$id" && die "the test lab's container was not removed"
  info "lab ok: it answers, runs unprivileged on a read-only root, and has no network"
}

summary() {
  echo >&2
  if [ "$dry" = 1 ]; then
    echo "Dry run: nothing was changed. Run again without --dry-run to install." >&2
  else
    echo "Installed." >&2
  fi
  cat >&2 <<EOF

Remaining steps on your reverse proxy and in Cloudflare:

  1. DNS: point $domain and *.$domain at your reverse proxy.
  2. Certificate: *.$domain needs its own wildcard certificate. A certificate
     for *.${domain#*.} does not cover it.
  3. Proxy: send $domain and *.$domain to this VM, with Host passed through and
     X-Real-IP set to the visitor's address:

       @necropsy host $domain *.$domain
       handle @necropsy {
           request_body {
               max_size 1MB
           }
           reverse_proxy http://$(hostname -I 2>/dev/null | awk '{print $1}'):8080 {
               flush_interval -1
               header_up X-Real-IP {remote_host}
               transport http {
                   read_timeout 86400s
                   write_timeout 86400s
               }
           }
       }

  4. Turnstile: in the Cloudflare dashboard, the widget's allowed hostnames
     must include $domain.

EOF
  if [ "$do_deploy" = 1 ] && [ "$dry" = 0 ]; then
    cat >&2 <<EOF
The first deploy created the admin account. Its credentials are shown once:

  docker logs necropsy-blue 2>&1 | grep -A4 'first run'     (necropsy-green if blue is idle)

EOF
  fi
  cat >&2 <<EOF
Day to day:
  sudo necropsy-deploy status | deploy <commit> | rollback
  systemctl status necropsy-spawner        journalctl -u necropsy-spawner -f
  docker ps --filter label=necropsy.range=1          (the running labs)
  sudoedit $CONF_DIR/necropsy.env          then deploy to apply
  CI deploys as $deploy_user over SSH, see docs/ci-cd.md
Run this script again to update the spawner and lab images and redeploy.
EOF
}

main() {
  parse_args "$@"
  validate_args
  preflight
  install_packages
  build_spawner
  install_images
  install_spawner_service
  selftest
  ensure_deploy_user
  install_deploy_program
  write_settings
  install_firewall
  deploy_site
  summary
}

# The tests source this file to call the helpers, and set NECROPSY_INSTALL_SOURCED.
if [ -z "${NECROPSY_INSTALL_SOURCED:-}" ]; then
  main "$@"
fi
