#!/usr/bin/env bash
# Zero-downtime (blue-green) deploys of necropsy with Docker.
#
#   deploy.sh <image>     start <image> as the idle color, switch traffic to it
#                         once healthy, then stop the old color
#   deploy.sh --rollback  switch back to the previous release (still on disk)
#   deploy.sh --status    show the live color, images and container health
#
# The image must already exist locally (docker load, or docker build). Paths
# come from the XDG variables, with the usual defaults under $HOME; nothing is
# hardcoded. See docs/docker.md.
#
# Deploy-time settings (all optional) go in the environment or in
# $XDG_CONFIG_HOME/necropsy/deploy.env:
#   NECROPSY_PORT          loopback port the proxy listens on      (8080)
#   NECROPSY_SUBNET        private subnet of the containers        (172.30.30.0/24)
#   NECROPSY_EDGE_TRUSTED  your own proxy's address as the container proxy sees
#                          it, comma-separated       (the subnet's gateway,127.0.0.1)
#   DRAIN_SECONDS          how long the old color keeps finishing requests (15)
#   HEALTH_TIMEOUT         seconds to wait for a new color to be healthy   (90)
#
# The app's own settings are in $XDG_CONFIG_HOME/necropsy/necropsy.env, the
# same file the systemd install uses.

set -euo pipefail

die() { echo "error: $*" >&2; exit 1; }
log() { echo "==> $*"; }

case "${1:-}" in
  ""|-h|--help)
    sed -n '2,22p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
    [ -n "${1:-}" ] && exit 0
    exit 2
    ;;
esac

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
config_dir="${XDG_CONFIG_HOME:-$HOME/.config}/necropsy"
bg_state="${XDG_STATE_HOME:-$HOME/.local/state}/necropsy/bluegreen"
proxy_dir="$bg_state/proxy"
images_env="$bg_state/images.env"
active_file="$bg_state/active"

# Optional deploy-time settings. The file is the deploy user's own.
if [ -f "$config_dir/deploy.env" ]; then
  set -a
  # shellcheck disable=SC1091
  . "$config_dir/deploy.env"
  set +a
fi

export NECROPSY_ENV_FILE="${NECROPSY_ENV_FILE:-$config_dir/necropsy.env}"
export NECROPSY_PROXY_DIR="$proxy_dir"
export NECROPSY_PORT="${NECROPSY_PORT:-8080}"
export NECROPSY_SUBNET="${NECROPSY_SUBNET:-172.30.30.0/24}"
drain_seconds="${DRAIN_SECONDS:-15}"
health_timeout="${HEALTH_TIMEOUT:-90}"

# The first usable address of the subnet is the Docker bridge gateway, which is
# who the proxy sees when your own proxy connects to the published port.
subnet_base="${NECROPSY_SUBNET%%/*}"
gateway="${subnet_base%.*}.$(( ${subnet_base##*.} + 1 ))"
edge_trusted="${NECROPSY_EDGE_TRUSTED:-$gateway,127.0.0.1}"

dc() { docker compose -f "$here/compose.yaml" --env-file "$images_env" "$@"; }

# ---- checks -----------------------------------------------------------------

case "$NECROPSY_PORT" in
  ''|*[!0-9]*) die "NECROPSY_PORT must be a number" ;;
esac
[[ "$NECROPSY_SUBNET" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+/[0-9]+$ ]] \
  || die "NECROPSY_SUBNET must look like 172.30.30.0/24"
[[ "$drain_seconds" =~ ^[0-9]+$ && "$health_timeout" =~ ^[0-9]+$ ]] \
  || die "DRAIN_SECONDS and HEALTH_TIMEOUT must be numbers"
# These end up inside an nginx config, so accept only addresses and ranges.
IFS=',' read -r -a edge_list <<< "$edge_trusted"
for e in "${edge_list[@]}"; do
  [[ "$e" =~ ^[0-9a-fA-F:.]+(/[0-9]+)?$ ]] || die "NECROPSY_EDGE_TRUSTED has an invalid entry: $e"
done

command -v docker >/dev/null || die "docker not found"
docker compose version >/dev/null 2>&1 || die "the docker compose plugin is required"
docker info >/dev/null 2>&1 || die "cannot talk to Docker (is the daemon running, and is this user allowed to use it?)"

mkdir -p "$bg_state" "$proxy_dir" "$config_dir"
touch "$images_env"

# ---- state ------------------------------------------------------------------

active_color() { [ -f "$active_file" ] && cat "$active_file" || true; }
other_color() { if [ "$1" = blue ]; then echo green; else echo blue; fi; }
container_of() { echo "necropsy-$1"; }

image_of() { # color
  local key
  key="$(echo "$1" | tr '[:lower:]' '[:upper:]')_IMAGE"
  { grep "^$key=" "$images_env" || true; } | tail -n 1 | cut -d= -f2-
}

set_image() { # color image
  local color="$1" image="$2" blue green
  blue="$(image_of blue)"
  green="$(image_of green)"
  if [ "$color" = blue ]; then blue="$image"; else green="$image"; fi
  {
    [ -z "$blue" ] || echo "BLUE_IMAGE=$blue"
    [ -z "$green" ] || echo "GREEN_IMAGE=$green"
  } > "$images_env.new"
  mv -f "$images_env.new" "$images_env"
}

write_proxy_conf() { # color
  {
    for e in "${edge_list[@]}"; do echo "set_real_ip_from $e;"; done
  } > "$proxy_dir/edge.conf.new"
  echo "server $(container_of "$1"):8080;" > "$proxy_dir/upstream.conf.new"
  # Rename into place, so nginx never reads a half-written file.
  mv -f "$proxy_dir/edge.conf.new" "$proxy_dir/edge.conf"
  mv -f "$proxy_dir/upstream.conf.new" "$proxy_dir/upstream.conf"
}

proxy_running() { [ "$(docker inspect -f '{{.State.Running}}' necropsy-proxy 2>/dev/null || true)" = true ]; }

wait_healthy() { # container
  local c="$1" waited=0 status
  while [ "$waited" -lt "$health_timeout" ]; do
    status="$(docker inspect -f '{{if .State.Running}}{{.State.Health.Status}}{{else}}stopped{{end}}' "$c" 2>/dev/null || echo missing)"
    case "$status" in
      healthy) return 0 ;;
      unhealthy|stopped|missing) return 1 ;;
    esac
    sleep 1
    waited=$((waited + 1))
  done
  return 1
}

# Through the proxy, the way real traffic arrives.
live_check() {
  command -v curl >/dev/null || { echo "note: curl not found, skipping the request through the proxy" >&2; return 0; }
  local i
  for i in 1 2 3 4 5 6 7 8 9 10; do
    if curl -fsS --max-time 5 "http://127.0.0.1:$NECROPSY_PORT/healthz" >/dev/null 2>&1; then return 0; fi
    sleep 1
  done
  return 1
}

# ---- one switch -------------------------------------------------------------

# Starts $target from $image (if given), waits for it, points the proxy at it,
# and stops $old after a drain. On any failure before traffic moves, the live
# color is left exactly as it was.
switch() { # target old image-or-empty
  local target="$1" old="$2" image="${3:-}" prev_images
  prev_images="$(cat "$images_env")"

  if [ -n "$image" ]; then set_image "$target" "$image"; fi
  log "starting $target ($(image_of "$target"))"
  dc up -d --no-deps --force-recreate "necropsy-$target" >/dev/null
  if ! wait_healthy "$(container_of "$target")"; then
    echo "--- last log lines from $target:" >&2
    docker logs --tail 40 "$(container_of "$target")" >&2 || true
    dc stop -t 5 "necropsy-$target" >/dev/null 2>&1 || true
    printf '%s\n' "$prev_images" > "$images_env"
    die "$target did not become healthy. Traffic stays on ${old:-nothing}."
  fi

  log "$target is healthy, switching traffic"
  local prev_upstream=""
  [ -f "$proxy_dir/upstream.conf" ] && prev_upstream="$(cat "$proxy_dir/upstream.conf")"
  write_proxy_conf "$target"
  if proxy_running; then
    if ! dc exec -T proxy nginx -t >/dev/null 2>&1 || ! dc exec -T proxy nginx -s reload >/dev/null 2>&1; then
      restore_proxy "$prev_upstream" "$old"
      dc stop -t 5 "necropsy-$target" >/dev/null 2>&1 || true
      printf '%s\n' "$prev_images" > "$images_env"
      die "the proxy rejected the new configuration. Traffic stays on ${old:-nothing}."
    fi
  else
    dc up -d --no-deps proxy >/dev/null
  fi

  if ! live_check; then
    restore_proxy "$prev_upstream" "$old"
    dc stop -t 5 "necropsy-$target" >/dev/null 2>&1 || true
    printf '%s\n' "$prev_images" > "$images_env"
    die "the site did not answer through the proxy after the switch. Traffic is back on ${old:-nothing}."
  fi

  echo "$target" > "$active_file.new" && mv -f "$active_file.new" "$active_file"
  echo "$(date -u +%FT%TZ) live=$target image=$(image_of "$target")" >> "$bg_state/history.log"

  if [ -n "$old" ]; then
    log "draining $old for ${drain_seconds}s"
    sleep "$drain_seconds"
    dc stop -t 20 "necropsy-$old" >/dev/null
  fi
  log "live: $target"
}

# Puts the proxy back on the previous color after a failed switch.
restore_proxy() { # previous-upstream-line old
  [ -n "$2" ] || return 0
  write_proxy_conf "$2"
  if [ -n "$1" ] && proxy_running; then
    dc exec -T proxy nginx -s reload >/dev/null 2>&1 || true
  fi
}

prune_images() { # repository
  local repo="$1" keep ref
  keep="$(image_of blue) $(image_of green)"
  docker image ls "$repo" --format '{{.Repository}}:{{.Tag}}' | while read -r ref; do
    case " $keep " in *" $ref "*) ;; *) docker rmi "$ref" >/dev/null 2>&1 || true ;; esac
  done
}

ensure_env_file() {
  [ -f "$NECROPSY_ENV_FILE" ] && return 0
  local example="$here/../../.env.example"
  [ -f "$example" ] || die "no settings file at $NECROPSY_ENV_FILE and no .env.example to start from"
  install -m 600 "$example" "$NECROPSY_ENV_FILE"
  log "created $NECROPSY_ENV_FILE with defaults. Edit it and deploy again to apply changes."
}

# ---- commands ---------------------------------------------------------------

status() {
  echo "live:    $(active_color || true)"
  echo "blue:    $(image_of blue)"
  echo "green:   $(image_of green)"
  echo "port:    127.0.0.1:$NECROPSY_PORT"
  local c
  for c in necropsy-blue necropsy-green necropsy-proxy; do
    printf '%-16s %s\n' "$c" "$(docker inspect -f '{{.State.Status}} {{if .State.Health}}({{.State.Health.Status}}){{end}}' "$c" 2>/dev/null || echo absent)"
  done
}

case "${1:-}" in
  --status)
    status
    exit 0
    ;;
esac

command -v flock >/dev/null || die "flock not found (install util-linux)"
exec 9> "$bg_state/lock"
flock -n 9 || die "another deploy is already running"

case "$1" in
  --rollback)
    old="$(active_color)"
    [ -n "$old" ] || die "nothing is deployed yet"
    target="$(other_color "$old")"
    image="$(image_of "$target")"
    [ -n "$image" ] || die "there is no previous release to go back to"
    docker image inspect "$image" >/dev/null 2>&1 || die "the previous image $image is gone"
    log "rolling back from $old to $target"
    switch "$target" "$old" ""
    ;;
  -*)
    die "unknown option: $1"
    ;;
  *)
    image="$1"
    docker image inspect "$image" >/dev/null 2>&1 \
      || die "image not found locally: $image (docker load it or docker build it first)"
    ensure_env_file
    old="$(active_color)"
    if [ -z "$old" ]; then target=blue; else target="$(other_color "$old")"; fi
    log "deploying $image: ${old:-nothing} -> $target"
    switch "$target" "$old" "$image"
    prune_images "${image%:*}"
    echo
    echo "Logs:    docker logs -f $(container_of "$target")"
    echo "Status:  $here/deploy.sh --status"
    echo "First deploy? The admin credentials were printed once to that log:"
    echo "  docker logs $(container_of "$target") 2>&1 | grep -A4 'first run'"
    ;;
esac
