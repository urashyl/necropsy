# shellcheck shell=bash
# Shared by install-site.sh and install-range.sh. Sourced, not run.
#
# Everything that changes the machine goes through `run` or `write_file`, so that
# --dry-run can show it instead of doing it. The scripts set `dry` (1 for a dry
# run) before calling these.

die() { echo "error: $*" >&2; exit 1; }
info() { echo "==> $*" >&2; }
warn() { echo "warning: $*" >&2; }

dry="${dry:-0}"

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

# ---- validation -----------------------------------------------------------------

valid_domain() { [[ "$1" =~ ^([a-z0-9]([a-z0-9-]*[a-z0-9])?\.)+[a-z]{2,}$ ]]; }
valid_addr() { [[ "$1" =~ ^[0-9a-fA-F:.]+$ ]] && [[ "$1" == *.* || "$1" == *:* ]]; }
valid_ipv4() { [[ "$1" =~ ^([0-9]{1,3}\.){3}[0-9]{1,3}$ ]] && ! [[ "$1" =~ (^|\.)(25[6-9]|2[6-9][0-9]|[3-9][0-9][0-9])(\.|$) ]]; }
valid_branch() { [[ "$1" =~ ^[A-Za-z0-9][A-Za-z0-9._/-]{0,99}$ ]] && [[ "$1" != *..* ]]; }
valid_user() { [[ "$1" =~ ^[a-z_][a-z0-9_-]*$ ]] && [ "$1" != root ]; }
valid_tenant() { [[ "$1" =~ ^[a-z][a-z0-9]{0,15}$ ]]; }

# ---- settings files -------------------------------------------------------------

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

# ---- packages and Docker --------------------------------------------------------

apt_available() { command -v apt-get >/dev/null 2>&1; }

# Installs what is missing from the tools named as tool:package pairs, then
# Docker (with the compose plugin) if it is not there.
install_packages() { # tool:package ...
  local need=() tool pkg pair
  for pair in "$@"; do
    tool="${pair%%:*}" pkg="${pair##*:}"
    command -v "$tool" >/dev/null 2>&1 || need+=("$pkg")
  done
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
  write_file /etc/apt/sources.list.d/docker.list 644 <<DOCKERLIST
deb [arch=$arch signed-by=/etc/apt/keyrings/docker.asc] https://download.docker.com/linux/$id $codename stable
DOCKERLIST
  run env DEBIAN_FRONTEND=noninteractive apt-get update -qq
  run env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
    docker-ce docker-ce-cli containerd.io docker-compose-plugin
}

# The checkout the script runs from, and its repository URL as https (from this
# checkout's origin, unless given).
checkout_repo_url() { # given-url -> prints the https URL
  if [ -n "$1" ]; then echo "$1"; return; fi
  local origin
  origin="$(git -C "$repo_root" remote get-url origin 2>/dev/null || true)"
  origin="${origin%.git}"
  case "$origin" in
    git@github.com:*) origin="https://github.com/${origin#git@github.com:}" ;;
  esac
  [[ "$origin" =~ ^https://[A-Za-z0-9._/-]+$ ]] \
    || die "cannot work out an https repository URL from this checkout's origin ($origin): pass --repo"
  echo "$origin"
}
