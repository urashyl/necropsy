#!/usr/bin/env bash
# Installs the whole of necropsy on a dedicated VM: the site (blue-green, in
# Docker), the live-puzzle range (spawner and labs), the settings, a firewall
# rule for the site's ports, and CI deploys. Production, and optionally a dev
# stack on the same VM, are installed as separate environments. Run as root,
# from a checkout of the repository, on the VM:
#
#   sudo deploy/install-vm.sh --domain necropsy.urashyl.dev --proxy-ip 192.168.100.5 \
#        --dev-domain necropsy-dev.urashyl.dev \
#        --turnstile-site-key 0x4AAA... --turnstile-secret-file /root/ts-secret \
#        --smtp-host smtp.example.com ...
#
# Secrets are never taken as options (they would show up in `ps` and in your
# shell history): pass them in the environment, or by file.
#   NECROPSY_TURNSTILE_SECRET   or  --turnstile-secret-file FILE
#   NECROPSY_SMTP_PASSWORD
#
# It is safe to run again: it updates what changed, keeps your settings, and
# redeploys. Use --dry-run to see everything it would do first.
#
# Each environment is a complete stack with its own names, so they cannot touch
# each other:
#
#                       production            dev (with --dev-domain)
#   stack name          necropsy              necropsy-dev
#   branch built        main                  dev
#   deploy user         deploy                deploy-dev
#   deploy program      necropsy-deploy       necropsy-dev-deploy
#   containers          necropsy-blue/...     necropsy-dev-blue/...
#   site port           8080                  8081
#   spawner unit        necropsy-spawner      necropsy-dev-spawner
#   settings            /etc/necropsy/        /etc/necropsy-dev/
#
# What it does:
#   1. installs what is missing (Docker from Docker's own apt repository on
#      Debian or Ubuntu; git, curl, iptables and friends)
#   2. builds the spawner program (inside a throwaway Rust container, so the VM
#      needs no Rust, and as a static binary, so it runs on any distribution)
#      and installs it once, shared by the environments
#   3. for each environment: its spawner service and a starting lab allowlist,
#      its unprivileged deploy user, its blue-green deploy program
#      (deploy/bluegreen/install-system.sh --name), and its settings
#   4. restricts the sites' ports to your reverse proxy
#   5. deploys the newest commit of each environment's branch. A deploy builds
#      the site and the lab images, restarts the spawner, and tests a real lab
#      for every image before any traffic moves (deploy/range/selftest.sh)
#
# After that, pushes deploy through CI (docs/ci-cd.md): main to production, dev
# to the dev stack, each over its own SSH user and sudo rule.
#
# Options:
#   --domain NAME          production's public name, e.g. necropsy.urashyl.dev
#                          (required). Live puzzles are served at *.NAME
#   --proxy-ip ADDR        the address your reverse proxy connects from. Only it
#                          may reach the sites' ports, and only it is believed
#                          about visitors' addresses (required, unless
#                          --proxy-on-host)
#   --proxy-on-host        the reverse proxy runs on this VM, on loopback
#   --repo URL             https URL to build releases from (default: this
#                          checkout's origin). It must be reachable without a login
#   --branch NAME          production's branch (main)
#   --deploy-user NAME     production's deploy account (deploy). Created if missing
#   --dev-domain NAME      also install a dev stack, served at NAME and *.NAME.
#                          Use a name that is not under --domain, so the two
#                          wildcard certificates stay simple
#   --dev-branch NAME      the dev stack's branch (dev)
#   --dev-deploy-user NAME the dev stack's deploy account (deploy-dev)
#   --turnstile-site-key K Cloudflare Turnstile site key (public), shared
#   --turnstile-secret-file FILE
#                          file holding the Turnstile secret key
#   --smtp-host H  --smtp-port P  --smtp-user U  --smtp-from "name <addr>"
#                          outgoing email (accounts need it), shared. TLS is
#                          STARTTLS on 587 unless you edit the settings file
#   --max-instances N      most labs at once for production (default: this VM's
#                          memory, see docs/range.md; with a dev stack, two
#                          thirds of it)
#   --dev-max-instances N  the same for dev (the remaining third)
#   --no-deploy            do everything except deploying the sites
#   --no-selftest          do not test the labs on deploy
#   --no-firewall          do not touch the firewall
#   --dry-run              print what would happen and change nothing
#   -h, --help
#
# Files, per environment NAME (necropsy, and necropsy-dev):
#   /etc/NAME/necropsy.env     the site's settings (mode 600)
#   /etc/NAME/deploy.env       deploy options (ports, labs, branch)
#   /etc/NAME/images.toml      the lab allowlist (written by each deploy)
#   /etc/systemd/system/NAME-spawner.service
#   /usr/local/bin/NAME-deploy
#   /run/NAME/                 the spawner socket and lab sockets (tmpfs)
# and, shared: /usr/local/bin/necropsy-spawner and necropsy-firewall.service.

set -euo pipefail
shopt -s inherit_errexit

die() { echo "error: $*" >&2; exit 1; }
info() { echo "==> $*" >&2; }
warn() { echo "warning: $*" >&2; }
usage() { sed -n '2,200p' "${BASH_SOURCE[0]}" | sed -n '1,/^set -euo/p' | sed '$d' | sed 's/^# \{0,1\}//'; }

# ---- settings ----------------------------------------------------------------

domain="" proxy_ip="" proxy_on_host=0 repo="" branch=main deploy_user=deploy
dev_domain="" dev_branch=dev dev_user=deploy-dev
site_key="" secret_file="" smtp_host="" smtp_port="" smtp_user="" smtp_from=""
max_instances="" dev_max_instances="" do_deploy=1 do_selftest=1 do_firewall=1 dry=0

CONF_ROOT=/etc
BIN_DIR=/usr/local/bin
LIB_DIR=/usr/local/lib/necropsy
SYSTEMD_DIR=/etc/systemd/system
# Inside the app container the spawner is always here; the host directory that
# is mounted there is /run/<stack name>.
CONTAINER_SOCKET=/run/necropsy/spawner.sock
CONTAINER_RANGE_DIR=/run/necropsy/range

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

# With production and dev on one VM, production gets two thirds of the labs.
split_instances() { # total prod|dev
  local prod=$(( ($1 * 2 + 2) / 3 ))
  [ "$prod" -ge 2 ] || prod=2
  local dev=$(( $1 - prod ))
  [ "$dev" -ge 2 ] || dev=2
  if [ "$2" = prod ]; then echo "$prod"; else echo "$dev"; fi
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
valid_branch() { [[ "$1" =~ ^[A-Za-z0-9][A-Za-z0-9._/-]{0,99}$ ]] && [[ "$1" != *..* ]]; }
valid_user() { [[ "$1" =~ ^[a-z_][a-z0-9_-]*$ ]] && [ "$1" != root ]; }

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
      --dev-domain) dev_domain="${2:?--dev-domain needs a name}"; shift 2 ;;
      --dev-branch) dev_branch="${2:?--dev-branch needs a name}"; shift 2 ;;
      --dev-deploy-user) dev_user="${2:?--dev-deploy-user needs a name}"; shift 2 ;;
      --turnstile-site-key) site_key="${2:?}"; shift 2 ;;
      --turnstile-secret-file) secret_file="${2:?}"; shift 2 ;;
      --smtp-host) smtp_host="${2:?}"; shift 2 ;;
      --smtp-port) smtp_port="${2:?}"; shift 2 ;;
      --smtp-user) smtp_user="${2:?}"; shift 2 ;;
      --smtp-from) smtp_from="${2:?}"; shift 2 ;;
      --max-instances) max_instances="${2:?}"; shift 2 ;;
      --dev-max-instances) dev_max_instances="${2:?}"; shift 2 ;;
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
  valid_branch "$branch" || die "--branch must be a plain branch name"
  valid_user "$deploy_user" || die "--deploy-user must be a valid user name, and not root"
  local m
  for m in "$max_instances" "$dev_max_instances"; do
    [ -z "$m" ] || { [[ "$m" =~ ^[0-9]+$ ]] && [ "$m" -ge 1 ]; } || die "--max-instances must be a number"
  done
  if [ -n "$dev_domain" ]; then
    valid_domain "$dev_domain" || die "--dev-domain must be a plain lower-case domain name"
    [ "$dev_domain" != "$domain" ] || die "--dev-domain must differ from --domain"
    valid_branch "$dev_branch" || die "--dev-branch must be a plain branch name"
    valid_user "$dev_user" || die "--dev-deploy-user must be a valid user name, and not root"
    [ "$dev_user" != "$deploy_user" ] || die "production and dev need different deploy users, so a dev push cannot run production's deploy"
  elif [ -n "$dev_max_instances" ]; then
    die "--dev-max-instances needs --dev-domain"
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

# ---- machine-wide steps ----------------------------------------------------------

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
  info "production https://$domain (live puzzles at *.$domain), building $repo ($branch)${dev_domain:+; dev https://$dev_domain ($dev_branch)}"

  total_mb=$(( $(awk '/^MemTotal:/ {print $2}' /proc/meminfo) / 1024 ))
  cpus_n="$(nproc)"
  info "this VM: $cpus_n vCPU, ${total_mb} MB"
  [ "$total_mb" -ge 2000 ] || warn "under 2 GB of memory: the site will run, but there is little room for labs"
}

apt_available() { command -v apt-get >/dev/null 2>&1; }

install_packages() {
  local need=()
  local tool pkg pair
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
# needs no Rust toolchain, and the result does not depend on its libc. One
# program, shared by the environments: only production deploys replace it.
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

# ---- one environment -----------------------------------------------------------
#
# select_env sets the e_* variables for "prod" or "dev"; every step below works
# on whichever is selected, so the two are made the same way.

e_name="" e_user="" e_branch="" e_domain="" e_port="" e_subnet="" e_max="" e_build=0 e_restart=""

select_env() { # prod | dev
  local total_max
  total_max="$(suggest_max_instances "$total_mb")"
  case "$1" in
    prod)
      e_name=necropsy e_user="$deploy_user" e_branch="$branch" e_domain="$domain"
      e_port=8080 e_subnet=172.30.30.0/24 e_build=1
      if [ -n "$dev_domain" ]; then
        e_restart=necropsy-dev-spawner
        e_max="${max_instances:-$(split_instances "$total_max" prod)}"
      else
        e_restart=""
        e_max="${max_instances:-$total_max}"
      fi
      ;;
    dev)
      e_name=necropsy-dev e_user="$dev_user" e_branch="$dev_branch" e_domain="$dev_domain"
      e_port=8081 e_subnet=172.30.31.0/24 e_build=0 e_restart=""
      e_max="${dev_max_instances:-$(split_instances "$total_max" dev)}"
      ;;
    *) die "internal error: no environment $1" ;;
  esac
}

install_env_spawner() {
  local unit="$e_name-spawner" conf="$CONF_ROOT/$e_name" images
  info "$e_name: the spawner service and a starting lab allowlist"
  run install -d -m 755 "$conf"
  # The first deploy replaces this with the labs it builds. Until then the
  # allowlist names images that do not exist yet, which only matters to a lab
  # that is asked for.
  images="$(mktemp)"
  sed -e "s|\"necropsy-range/\([a-z0-9-]*\):1\"|\"necropsy-range/$e_name/\1:initial\"|" \
      -e "s|^max_instances *=.*|max_instances = $e_max|" \
      "$repo_root/range/images.toml" > "$images"
  if [ "$cpus_n" -le 2 ]; then cap_cpus "$images" 0.25; fi
  if [ "$dry" = 1 ]; then
    echo "+ write $conf/images.toml (at most $e_max labs)" >&2
  else
    # Keep the allowlist a deploy wrote; only create it when there is none.
    [ -f "$conf/images.toml" ] || install -m 644 "$images" "$conf/images.toml"
  fi
  rm -f "$images"
  if [ "$dry" = 0 ]; then
    sed "s/@NAME@/$e_name/g" "$repo_root/deploy/range/necropsy-spawner.service.in" > "$SYSTEMD_DIR/$unit.service"
    chmod 644 "$SYSTEMD_DIR/$unit.service"
  else
    echo "+ write $SYSTEMD_DIR/$unit.service" >&2
  fi
  run systemctl daemon-reload
  run systemctl enable "$unit"
  run systemctl restart "$unit"
  if [ "$dry" = 0 ]; then wait_for_spawner "$unit" "/run/$e_name/spawner.sock"; fi
}

wait_for_spawner() { # unit socket
  local i
  for i in $(seq 1 50); do
    [ -S "$2" ] && break
    sleep 0.2
  done
  [ -S "$2" ] || die "$1 did not start. See: journalctl -u $1 -n 50"
  curl -fsS --unix-socket "$2" http://spawner/healthz >/dev/null \
    || die "$1 does not answer. See: journalctl -u $1 -n 50"
}

ensure_deploy_user() {
  if id -u "$e_user" >/dev/null 2>&1; then
    info "$e_name: deploy user $e_user exists"
    return
  fi
  info "$e_name: creating the deploy user $e_user (no password; add your CI's public key to ~$e_user/.ssh/authorized_keys)"
  run useradd --create-home --shell /bin/bash "$e_user"
}

install_deploy_program() {
  info "$e_name: installing the blue-green deploy program"
  run "$repo_root/deploy/bluegreen/install-system.sh" --name "$e_name" --user "$e_user" --repo "$repo" --branch "$e_branch"
}

write_settings() {
  info "$e_name: writing the settings"
  local conf="$CONF_ROOT/$e_name"
  if [ "$dry" = 1 ]; then
    echo "+ set in $conf/necropsy.env: NECROPSY_BASE_URL NECROPSY_LIVE_DOMAIN NECROPSY_SPAWNER_SOCKET NECROPSY_RANGE_DIR (and captcha and email if given)" >&2
    echo "+ set in $conf/deploy.env: NECROPSY_BIND NECROPSY_PORT NECROPSY_SUBNET NECROPSY_EDGE_TRUSTED RANGE_UNIT RANGE_MAX_INSTANCES ..." >&2
    return
  fi
  local env="$conf/necropsy.env" denv="$conf/deploy.env"
  [ -f "$env" ] || die "$env was not created by the deploy installer"
  set_env "$env" NECROPSY_BASE_URL "https://$e_domain"
  set_env "$env" NECROPSY_LIVE_DOMAIN "$e_domain"
  set_env "$env" NECROPSY_SPAWNER_SOCKET "$CONTAINER_SOCKET"
  set_env "$env" NECROPSY_RANGE_DIR "$CONTAINER_RANGE_DIR"

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
  set_env "$denv" NECROPSY_PORT "$e_port"
  set_env "$denv" NECROPSY_SUBNET "$e_subnet"
  # What each deploy does about the labs (deploy/bluegreen/deploy.sh).
  set_env "$denv" RANGE_UNIT "$e_name-spawner"
  set_env "$denv" RANGE_MAX_INSTANCES "$e_max"
  set_env "$denv" RANGE_BUILD_SPAWNER "$e_build"
  set_env "$denv" RANGE_RESTART_UNITS "$e_restart"
  set_env "$denv" RANGE_SELFTEST "$do_selftest"
  if [ "$cpus_n" -le 2 ]; then set_env "$denv" RANGE_CPUS_CAP 0.25; fi
  chmod 644 "$denv"
}

# Docker's published ports bypass ufw and firewalld, so the sites' ports are
# closed to everything but your proxy in Docker's own DOCKER-USER chain, by a
# small unit that runs after Docker starts. In that chain the port is the
# container's own (always 8080), so one rule covers every stack's published port.
install_firewall() {
  if [ "$do_firewall" = 0 ] || [ "$proxy_on_host" = 1 ]; then
    return
  fi
  if [[ "$proxy_ip" == *:* ]]; then
    warn "an IPv6 proxy address: not adding a firewall rule. Restrict the sites' ports to it yourself"
    return
  fi
  info "restricting the sites' ports to $proxy_ip"
  run install -d -m 755 "$LIB_DIR"
  write_file "$LIB_DIR/firewall.sh" 755 <<'EOF'
#!/bin/sh
# Only the reverse proxy may reach the sites' published ports. In the DOCKER-USER
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
Description=necropsy: only the reverse proxy may reach the sites
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
  [ "$do_deploy" = 1 ] || { info "$e_name: skipping the deploy (--no-deploy). Later: sudo $e_name-deploy deploy <commit on $e_branch>"; return 0; }
  info "$e_name: deploying the newest commit of $e_branch"
  local sha
  if [ "$dry" = 1 ]; then
    echo "+ $e_name-deploy deploy <newest commit of $e_branch at $repo>" >&2
    return 0
  fi
  sha="$(git ls-remote "$repo" "refs/heads/$e_branch" | awk '{print $1}' | head -n 1)"
  [[ "$sha" =~ ^[0-9a-f]{40}$ ]] \
    || die "branch $e_branch was not found at $repo. Push it there first, or pick another branch"
  "$BIN_DIR/$e_name-deploy" deploy "$sha"
}

proxy_block() { # domain port
  cat <<EOF
       @$e_name host $1 *.$1
       handle @$e_name {
           request_body {
               max_size 1MB
           }
           reverse_proxy http://$(hostname -I 2>/dev/null | awk '{print $1}'):$2 {
               flush_interval -1
               header_up X-Real-IP {remote_host}
               transport http {
                   read_timeout 86400s
                   write_timeout 86400s
               }
           }
       }
EOF
}

envs() { if [ -n "$dev_domain" ]; then echo "prod dev"; else echo "prod"; fi; }

summary() {
  echo >&2
  if [ "$dry" = 1 ]; then
    echo "Dry run: nothing was changed. Run again without --dry-run to install." >&2
  else
    echo "Installed." >&2
  fi
  local which
  {
    echo
    echo "Remaining steps on your reverse proxy and in Cloudflare:"
    echo
    echo "  1. DNS: point each site's name and its wildcard at your reverse proxy."
    echo "  2. Certificates: each *.<site name> needs its own wildcard certificate. A"
    echo "     certificate for *.${domain#*.} does not cover *.$domain."
    echo "  3. Proxy: send each site to this VM, with Host passed through and X-Real-IP set"
    echo "     to the visitor's address:"
    for which in $(envs); do
      select_env "$which"
      echo
      echo "     $e_name ($e_domain, branch $e_branch):"
      proxy_block "$e_domain" "$e_port"
    done
    echo
    echo "  4. Turnstile: in the Cloudflare dashboard, the widget's allowed hostnames must"
    echo "     include ${domain}${dev_domain:+ and $dev_domain}."
    echo "  5. CI: for each GitHub environment, set the secrets in docs/ci-cd.md. They share"
    echo "     DEPLOY_HOST, DEPLOY_PORT and DEPLOY_KNOWN_HOSTS; DEPLOY_USER and the SSH key differ"
    echo "     ($deploy_user for production${dev_domain:+, $dev_user for dev}). Add each key's public half to that"
    echo "     user's ~/.ssh/authorized_keys on this VM."
    echo
    if [ "$do_deploy" = 1 ] && [ "$dry" = 0 ]; then
      echo "The first deploy of each site created its admin account. The credentials are shown once:"
      echo
      for which in $(envs); do
        select_env "$which"
        echo "  docker logs $e_name-blue 2>&1 | grep -A4 'first run'     ($e_name-green if blue is idle)"
      done
      echo
    fi
    echo "Day to day:"
    for which in $(envs); do
      select_env "$which"
      echo "  sudo $e_name-deploy status | deploy <commit> | rollback     ($e_name)"
      echo "  systemctl status $e_name-spawner     docker ps --filter label=necropsy.range=1"
      echo "  sudoedit $CONF_ROOT/$e_name/necropsy.env     then deploy to apply"
    done
    echo "Run this script again to update the shared spawner program and the firewall rule."
  } >&2
}

main() {
  parse_args "$@"
  validate_args
  preflight
  install_packages
  build_spawner
  local which
  for which in $(envs); do
    select_env "$which"
    install_env_spawner
    ensure_deploy_user
    install_deploy_program
    write_settings
  done
  install_firewall
  for which in $(envs); do
    select_env "$which"
    deploy_site
  done
  summary
}

# The tests source this file to call the helpers, and set NECROPSY_INSTALL_SOURCED.
if [ -z "${NECROPSY_INSTALL_SOURCED:-}" ]; then
  main "$@"
fi
