#!/usr/bin/env bash
# necropsy-deploy: zero-downtime (blue-green) deploys of necropsy with Docker.
#
#   necropsy-deploy deploy <commit>   build that commit (it must be on the
#                                     configured branch, main by default),
#                                     start it as the idle color, switch
#                                     traffic once it is healthy, stop the old
#   necropsy-deploy rollback          switch back to the previous release
#   necropsy-deploy status            live color, images, container health
#
# This file is a template. install-system.sh bakes the paths below into a copy
# that it installs as a root-owned program (/usr/local/bin/necropsy-deploy by
# default), and allows one unprivileged user to run it through sudo without a
# password, and nothing else. See docs/docker.md.
#
# Because an unprivileged user can start it as root, it trusts nothing the
# caller controls: not the environment, not the arguments (checked below), not
# the files it reads (root-owned configuration only), and it builds only code
# that is on the branch and repository named in that configuration.
#
# Settings are read from $SYSCONFDIR/necropsy/deploy.env, a plain KEY=VALUE
# file that is parsed, never executed:
#   NECROPSY_REPO          https URL of the repository (required)
#   NECROPSY_BRANCH        the only branch whose commits are built  (main)
#   NECROPSY_BIND          host address the proxy container is published on
#                          (127.0.0.1; 0.0.0.0 for a reverse proxy on another host)
#   NECROPSY_PORT          host port the proxy container is published on (8080)
#   NECROPSY_SUBNET        private subnet of the containers          (172.30.30.0/24)
#   NECROPSY_EDGE_TRUSTED  your reverse proxy's address as the proxy container
#                          sees it, comma-separated     (subnet gateway,127.0.0.1)
#   DRAIN_SECONDS          how long the old color finishes requests  (15)
#   HEALTH_TIMEOUT         seconds to wait for a new color to be healthy (90)
# and, for a stack that runs live-puzzle labs (docs/range.md), where each deploy
# also builds the lab images, rewrites the lab allowlist, restarts the spawner
# and tests it before any traffic moves:
#   RANGE_UNIT             the spawner's systemd unit; setting it turns this on
#   RANGE_MAX_INSTANCES    most labs at once, written into the allowlist (20)
#   RANGE_CPUS_CAP         lower every lab's cpu limit to at most this
#   RANGE_BUILD_SPAWNER    1 to also rebuild the spawner program from the
#                          commit (production only, see docs/range.md)
#   RANGE_SPAWNER_BIN      where that program is installed (/usr/local/bin/necropsy-spawner)
#   RANGE_RESTART_UNITS    other spawner units to restart when it changes
#   RANGE_SELFTEST         0 to skip the lab test (1)
# The application's own settings are in $SYSCONFDIR/necropsy/necropsy.env.

set -euo pipefail
# Without this, a failure inside $(...) does not stop the script.
shopt -s inherit_errexit

# ---- baked in by install-system.sh ------------------------------------------
LIBDIR='@LIBDIR@'
SYSCONFDIR='@SYSCONFDIR@'
STATEDIR='@STATEDIR@'
SAFE_PATH='@SAFE_PATH@'
# 1 in a real install: root only, clean environment, https only, no `switch`.
# 0 only for the test suite.
SAFE_MODE='@SAFE_MODE@'
# The stack's name: necropsy for the only or production stack. Everything that
# would clash on a host running several (container and volume names, the image
# repository, the settings and state directories) follows it.
NAME='@NAME@'
# Where runtime files go (the spawner's socket and each lab's socket live in
# RUNDIR/NAME), /run on a real host.
RUNDIR='@RUNDIR@'
# -----------------------------------------------------------------------------

die() { echo "error: $*" >&2; exit 1; }
log() { echo "==> $*" >&2; }

[[ "$NAME" =~ ^[a-z][a-z0-9-]{0,30}$ ]] || { echo "error: bad stack name: $NAME" >&2; exit 1; }
case "$LIBDIR" in
  @*) die "this is the template. Run install-system.sh and use the installed program." ;;
esac

usage() {
  sed -n '2,31p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

case "${1:-}" in
  -h|--help) usage; exit 0 ;;
esac

if [ "$SAFE_MODE" = 1 ]; then
  [ "$(id -u)" -eq 0 ] || die "must run as root (through sudo)"
  # The environment comes from the unprivileged caller. Start over from a
  # known one, so DOCKER_HOST, GIT_*, LD_PRELOAD and the like cannot steer us.
  dirty=0
  for v in $(compgen -e); do
    case "$v" in PATH|HOME|LANG|PWD|OLDPWD|SHLVL|_) ;; *) dirty=1 ;; esac
  done
  [ "$PATH" = "$SAFE_PATH" ] || dirty=1
  if [ "$dirty" = 1 ]; then
    [ "${1:-}" != --cleaned ] || die "could not get a clean environment"
    exec env -i PATH="$SAFE_PATH" HOME=/root LANG=C "$(readlink -f "${BASH_SOURCE[0]}")" --cleaned "$@"
  fi
  [ "${1:-}" != --cleaned ] || shift
  umask 077
fi

config_dir="$SYSCONFDIR/$NAME"
bg_state="$STATEDIR/$NAME/bluegreen"
src_dir="$STATEDIR/$NAME/src.git"
deploy_conf="$config_dir/deploy.env"
compose_file="$LIBDIR/bluegreen/compose.yaml"
images_env="$bg_state/images.env"
active_file="$bg_state/active"
proxy_dir="$bg_state/proxy"

# ---- configuration (parsed, never sourced) -----------------------------------

NECROPSY_REPO="" NECROPSY_BRANCH=main NECROPSY_BIND=127.0.0.1 NECROPSY_PORT=8080 NECROPSY_SUBNET=172.30.30.0/24
NECROPSY_EDGE_TRUSTED="" DRAIN_SECONDS=15 HEALTH_TIMEOUT=90
RANGE_UNIT="" RANGE_MAX_INSTANCES=20 RANGE_CPUS_CAP="" RANGE_BUILD_SPAWNER=0
RANGE_SPAWNER_BIN=/usr/local/bin/necropsy-spawner RANGE_RESTART_UNITS="" RANGE_SELFTEST=1

# A file that someone other than root can write is a way to change what runs as
# root, so in a real install it must be root-owned and not writable by others.
trusted_file() {
  [ "$SAFE_MODE" = 1 ] || return 0
  local owner mode
  owner="$(stat -c %u "$1")"
  mode="$(stat -c %a "$1")"
  [ "$owner" = 0 ] && [ $(( 8#$mode & 8#022 )) -eq 0 ]
}

if [ -f "$deploy_conf" ]; then
  trusted_file "$deploy_conf" || die "$deploy_conf must be owned by root and not writable by others"
  while IFS='=' read -r key value || [ -n "$key" ]; do
    case "$key" in ''|'#'*) continue ;; esac
    value="${value%\"}"; value="${value#\"}"; value="${value%\'}"; value="${value#\'}"
    case "$key" in
      NECROPSY_REPO|NECROPSY_BRANCH|NECROPSY_BIND|NECROPSY_PORT|NECROPSY_SUBNET|NECROPSY_EDGE_TRUSTED|DRAIN_SECONDS|HEALTH_TIMEOUT|RANGE_UNIT|RANGE_MAX_INSTANCES|RANGE_CPUS_CAP|RANGE_BUILD_SPAWNER|RANGE_SPAWNER_BIN|RANGE_RESTART_UNITS|RANGE_SELFTEST)
        printf -v "$key" '%s' "$value" ;;
    esac
  done < "$deploy_conf"
fi

export NECROPSY_NAME="$NAME"
export NECROPSY_ENV_FILE="$config_dir/necropsy.env"
# Where this stack's spawner keeps its sockets, mounted into the app container.
export NECROPSY_RANGE_HOST_DIR="$RUNDIR/$NAME"
export NECROPSY_PROXY_DIR="$proxy_dir"
export NECROPSY_BIND NECROPSY_PORT NECROPSY_SUBNET

# The first usable address of the subnet is the Docker bridge gateway, which is
# who the proxy container sees when your own proxy connects to the published port.
subnet_base="${NECROPSY_SUBNET%%/*}"
gateway="${subnet_base%.*}.$(( ${subnet_base##*.} + 1 ))"
edge_trusted="${NECROPSY_EDGE_TRUSTED:-$gateway,127.0.0.1}"

dc() { docker compose -f "$compose_file" --env-file "$images_env" "$@"; }

# ---- checks -----------------------------------------------------------------

[[ "$NECROPSY_PORT" =~ ^[0-9]+$ ]] && [ "$NECROPSY_PORT" -ge 1 ] && [ "$NECROPSY_PORT" -le 65535 ] \
  || die "NECROPSY_PORT must be a number from 1 to 65535"
# A plain IPv4 or IPv6 address, no brackets, zone or port: it goes to compose.
octet='(25[0-5]|2[0-4][0-9]|1[0-9][0-9]|[1-9]?[0-9])'
if [[ "$NECROPSY_BIND" =~ ^$octet\.$octet\.$octet\.$octet$ ]]; then
  :
elif [[ "$NECROPSY_BIND" == *:* && "$NECROPSY_BIND" =~ ^[0-9a-fA-F:]{2,39}$ ]]; then
  :
else
  die "NECROPSY_BIND must be an IPv4 or IPv6 address, like 127.0.0.1 or 0.0.0.0"
fi
# Where the deploy itself reaches the proxy: a wildcard is reachable on loopback.
case "$NECROPSY_BIND" in
  0.0.0.0) check_host=127.0.0.1 ;;
  ::|0:0:0:0:0:0:0:0) check_host='[::1]' ;;
  *:*) check_host="[$NECROPSY_BIND]" ;;
  *) check_host="$NECROPSY_BIND" ;;
esac
[[ "$NECROPSY_SUBNET" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+/[0-9]+$ ]] \
  || die "NECROPSY_SUBNET must look like 172.30.30.0/24"
[[ "$DRAIN_SECONDS" =~ ^[0-9]+$ && "$HEALTH_TIMEOUT" =~ ^[0-9]+$ ]] \
  || die "DRAIN_SECONDS and HEALTH_TIMEOUT must be numbers"
# The range settings end up in systemctl and docker commands and in a file.
unit_name='^[a-z][a-z0-9-]{0,40}$'
if [ -n "$RANGE_UNIT" ]; then
  [[ "$RANGE_UNIT" =~ $unit_name ]] || die "RANGE_UNIT is not a plain unit name"
  for u in $RANGE_RESTART_UNITS; do
    [[ "$u" =~ $unit_name ]] || die "RANGE_RESTART_UNITS has an invalid unit: $u"
  done
  [[ "$RANGE_MAX_INSTANCES" =~ ^[0-9]{1,4}$ ]] && [ "$RANGE_MAX_INSTANCES" -ge 1 ] \
    || die "RANGE_MAX_INSTANCES must be a number from 1 to 9999"
  [ -z "$RANGE_CPUS_CAP" ] || [[ "$RANGE_CPUS_CAP" =~ ^[0-9]+(\.[0-9]+)?$ ]] || die "RANGE_CPUS_CAP must be a number like 0.25"
  [[ "$RANGE_BUILD_SPAWNER" =~ ^[01]$ && "$RANGE_SELFTEST" =~ ^[01]$ ]] || die "RANGE_BUILD_SPAWNER and RANGE_SELFTEST must be 0 or 1"
  [[ "$RANGE_SPAWNER_BIN" =~ ^/[A-Za-z0-9._/+-]+$ ]] || die "RANGE_SPAWNER_BIN must be an absolute path"
fi
# These end up inside the proxy config, so accept only addresses and ranges.
IFS=',' read -r -a edge_list <<< "$edge_trusted"
for e in "${edge_list[@]}"; do
  [[ "$e" =~ ^[0-9a-fA-F:.]+(/[0-9]+)?$ ]] || die "NECROPSY_EDGE_TRUSTED has an invalid entry: $e"
done

# An image reference is only ever one of ours.
valid_image() { [[ "$1" =~ ^$NAME:[A-Za-z0-9_.-]{1,64}$ ]]; }

# ---- state ------------------------------------------------------------------

active_color() { [ -f "$active_file" ] && cat "$active_file" || true; }
other_color() { if [ "$1" = blue ]; then echo green; else echo blue; fi; }
container_of() { echo "$NAME-$1"; }

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
  echo "trusted_proxies static ${edge_list[*]}" > "$proxy_dir/edge.caddy.new"
  echo "to $(container_of "$1"):8080" > "$proxy_dir/upstream.caddy.new"
  # Rename into place, so Caddy never reads a half-written file.
  mv -f "$proxy_dir/edge.caddy.new" "$proxy_dir/edge.caddy"
  mv -f "$proxy_dir/upstream.caddy.new" "$proxy_dir/upstream.caddy"
}

proxy_running() { [ "$(docker inspect -f '{{.State.Running}}' "$NAME-proxy" 2>/dev/null || true)" = true ]; }
# The address and port the running proxy is published on, as Docker has it.
proxy_published() {
  docker inspect -f '{{range $p, $b := .HostConfig.PortBindings}}{{range $b}}{{.HostIp}} {{.HostPort}}{{end}}{{end}}' \
    "$NAME-proxy" 2>/dev/null || true
}
# A changed NECROPSY_BIND or NECROPSY_PORT only takes effect on a new container.
proxy_current() { [ "$(proxy_published)" = "$NECROPSY_BIND $NECROPSY_PORT" ]; }

wait_healthy() { # container
  local c="$1" waited=0 status
  while [ "$waited" -lt "$HEALTH_TIMEOUT" ]; do
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
    if curl -fsS --max-time 5 "http://$check_host:$NECROPSY_PORT/healthz" >/dev/null 2>&1; then return 0; fi
    sleep 1
  done
  return 1
}

caddy_args=(--config /etc/caddy/Caddyfile --adapter caddyfile)
proxy_validate() { dc exec -T proxy caddy validate "${caddy_args[@]}" >/dev/null 2>&1; }
# Graceful: requests already in flight finish on the old upstream.
proxy_reload() { dc exec -T proxy caddy reload "${caddy_args[@]}" --address unix//tmp/caddy-admin.sock >/dev/null 2>&1; }

# Puts the proxy back on the previous color after a failed switch.
restore_proxy() { # old
  [ -n "$1" ] || return 0
  write_proxy_conf "$1"
  if proxy_running; then
    proxy_reload || true
  fi
}

# ---- one switch -------------------------------------------------------------

# Starts $target from $image (if given), waits for it, points the proxy at it,
# and stops $old after a drain. On any failure before traffic moves, the live
# color is left exactly as it was.
switch_traffic() { # target old image-or-empty
  local target="$1" old="$2" image="${3:-}" prev_images recreated=0
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
  write_proxy_conf "$target"
  if proxy_running && ! proxy_current; then
    # Traffic pauses for the moment the proxy restarts. Only when the setting changed.
    log "the proxy's address changed to $NECROPSY_BIND:$NECROPSY_PORT, recreating it"
    recreated=1
    dc up -d --no-deps --force-recreate proxy >/dev/null \
      || die "the proxy could not start on $NECROPSY_BIND:$NECROPSY_PORT (is that address on this host, and the port free?). Fix NECROPSY_BIND or NECROPSY_PORT in $deploy_conf and deploy again."
  elif proxy_running; then
    if ! proxy_validate || ! proxy_reload; then
      restore_proxy "$old"
      dc stop -t 5 "necropsy-$target" >/dev/null 2>&1 || true
      printf '%s\n' "$prev_images" > "$images_env"
      die "the proxy rejected the new configuration. Traffic stays on ${old:-nothing}."
    fi
  else
    dc up -d --no-deps proxy >/dev/null
  fi

  if ! live_check; then
    if [ "$recreated" = 1 ]; then
      die "the proxy was recreated on $NECROPSY_BIND:$NECROPSY_PORT but the site does not answer there. The old proxy is gone; fix NECROPSY_BIND or NECROPSY_PORT in $deploy_conf and deploy again."
    fi
    restore_proxy "$old"
    dc stop -t 5 "necropsy-$target" >/dev/null 2>&1 || true
    printf '%s\n' "$prev_images" > "$images_env"
    die "the site did not answer through the proxy after the switch. Traffic is back on ${old:-nothing}."
  fi

  echo "$target" > "$active_file.new" && mv -f "$active_file.new" "$active_file"
  echo "$(date -u +%FT%TZ) live=$target image=$(image_of "$target")" >> "$bg_state/history.log"

  if [ -n "$old" ]; then
    log "draining $old for ${DRAIN_SECONDS}s"
    sleep "$DRAIN_SECONDS"
    dc stop -t 20 "necropsy-$old" >/dev/null
  fi
  log "live: $target"
}

prune_images() {
  local keep ref
  keep="$(image_of blue) $(image_of green)"
  docker image ls "$NAME" --format '{{.Repository}}:{{.Tag}}' | while read -r ref; do
    case " $keep " in *" $ref "*) ;; *) docker rmi "$ref" >/dev/null 2>&1 || true ;; esac
  done
  # Build cache older than a week.
  docker builder prune -f --filter until=168h >/dev/null 2>&1 || true
}

ensure_env_file() {
  [ -f "$NECROPSY_ENV_FILE" ] && return 0
  [ -f "$LIBDIR/env.example" ] || die "no settings file at $NECROPSY_ENV_FILE and no example to start from"
  install -m 600 "$LIBDIR/env.example" "$NECROPSY_ENV_FILE"
  log "created $NECROPSY_ENV_FILE with defaults. Edit it, then deploy again to apply changes."
}

# ---- the range: labs and spawner ----------------------------------------------

range_unit_active() { systemctl is-active "$1" 2>/dev/null || true; }

# Puts the previous allowlist and spawner program back, restarts the spawner on
# them, and stops the deploy. Uses build_range's variables.
range_fail() { # message
  [ "$had_images" = 1 ] && mv -f "$images_file.prev" "$images_file"
  [ "$had_bin" = 1 ] && mv -f "$RANGE_SPAWNER_BIN.prev" "$RANGE_SPAWNER_BIN"
  local u
  for u in $RANGE_UNIT $([ "$changed_bin" = 1 ] && echo "$RANGE_RESTART_UNITS"); do
    systemctl restart "$u" >/dev/null 2>&1 || true
  done
  die "$1 The site was not changed, and the spawner is back on the previous allowlist."
}

# Builds this commit's lab images, rewrites the allowlist to point at them,
# optionally rebuilds the spawner, restarts it, and tests the result. Everything
# runs before traffic moves, so a broken lab stops the deploy with the live site
# untouched, and the previous allowlist and spawner are put back.
#
# The lab images are sandboxed containers like any other. The spawner program is
# different: it runs as root. So only a stack with RANGE_BUILD_SPAWNER=1
# (production, which builds only reviewed commits on main) ever replaces it.
build_range() { # build_dir full-sha
  local dir="$1" short="${2:0:12}" labdir lab tmp ref images_file="$config_dir/images.toml"
  local changed_bin=0 had_images=0 had_bin=0 u waited sock="$RUNDIR/$NAME/spawner.sock"
  [ -f "$dir/range/images.toml" ] || { log "this commit has no range, skipping it"; return 0; }

  for labdir in "$dir"/range/*/; do
    [ -f "$labdir/Dockerfile" ] || continue
    lab="$(basename "$labdir")"
    [[ "$lab" =~ ^[a-z0-9][a-z0-9-]*$ ]] || die "bad lab name: $lab"
    log "building lab $lab"
    docker build --pull -t "necropsy-range/$NAME/$lab:$short" "$labdir" >&2
  done

  # The allowlist: the commit's, pointed at the images just built, with this
  # stack's own limits on how many.
  tmp="$images_file.new"
  sed -e "s|\"necropsy-range/\([a-z0-9-]*\):1\"|\"necropsy-range/$NAME/\1:$short\"|" \
      -e "s|^max_instances *=.*|max_instances = $RANGE_MAX_INSTANCES|" \
      "$dir/range/images.toml" > "$tmp"
  if [ -n "$RANGE_CPUS_CAP" ]; then
    awk -v c="$RANGE_CPUS_CAP" '/^cpus *=/ { if ($3 + 0 > c + 0) $3 = c } { print }' "$tmp" > "$tmp.2"
    mv -f "$tmp.2" "$tmp"
  fi
  while read -r ref; do
    case "$ref" in
      "necropsy-range/$NAME/"*) docker image inspect "$ref" >/dev/null 2>&1 \
        || die "the allowlist names $ref, but this commit has no lab image for it" ;;
      *) die "the allowlist names an image that is not one of this commit's labs: $ref" ;;
    esac
  done < <(sed -n 's/^image *= *"\([^"]*\)".*/\1/p' "$tmp")

  if [ "$RANGE_BUILD_SPAWNER" = 1 ]; then
    local rust out
    rust="$(sed -n 's/^ARG RUST_VERSION=//p' "$dir/Dockerfile" | head -n 1)"
    [[ "$rust" =~ ^[0-9][0-9.]*$ ]] || die "cannot find the Rust version in this commit's Dockerfile"
    log "building the spawner"
    out="$(mktemp -d "$bg_state/spawner.XXXXXX")"
    docker run --rm \
      -v "$dir":/src:ro \
      -v necropsy-build-cargo:/usr/local/cargo/registry \
      -v necropsy-build-target:/target \
      -v "$out":/out \
      -e CARGO_TARGET_DIR=/target -w /src \
      "rust:$rust-alpine" \
      sh -c 'apk add --no-cache musl-dev >/dev/null && cargo build --release --locked -p necropsy-spawner && cp /target/release/necropsy-spawner /out/' >&2 \
      || { rm -rf "$out"; die "the spawner did not build"; }
    [ -s "$out/necropsy-spawner" ] || { rm -rf "$out"; die "the spawner did not build"; }
    if ! cmp -s "$out/necropsy-spawner" "$RANGE_SPAWNER_BIN"; then
      if [ -f "$RANGE_SPAWNER_BIN" ]; then cp -p "$RANGE_SPAWNER_BIN" "$RANGE_SPAWNER_BIN.prev"; had_bin=1; fi
      install -m 755 "$out/necropsy-spawner" "$RANGE_SPAWNER_BIN.new"
      mv -f "$RANGE_SPAWNER_BIN.new" "$RANGE_SPAWNER_BIN"
      changed_bin=1
    fi
    rm -rf "$out"
  fi

  # From here on a failure puts the previous allowlist (and program) back.
  if [ -f "$images_file" ]; then cp -p "$images_file" "$images_file.prev"; had_images=1; fi
  mv -f "$tmp" "$images_file"
  chmod 644 "$images_file"

  # The spawner always restarts: it reads its allowlist once, at start, and
  # labs already running are found again afterwards. The other stacks' spawners
  # restart only when the program they share changed.
  local units="$RANGE_UNIT"
  [ "$changed_bin" = 1 ] && units="$RANGE_UNIT $RANGE_RESTART_UNITS"
  for u in $units; do
    log "restarting $u"
    systemctl restart "$u" || range_fail "Could not restart $u."
  done

  if [ "$RANGE_SELFTEST" = 1 ]; then
    waited=0
    while [ ! -e "$sock" ] && [ "$waited" -lt 20 ]; do sleep 1; waited=$((waited + 1)); done
    [ -e "$sock" ] || range_fail "The spawner has no socket at $sock (systemctl status $RANGE_UNIT)."
    log "testing the labs"
    "$LIBDIR/range-selftest.sh" "$images_file" "$sock" "$RUNDIR/$NAME/range" >&2 \
      || range_fail "The labs failed their test."
  fi
  rm -f "$images_file.prev" "$RANGE_SPAWNER_BIN.prev"

  # Keep this commit's lab images and the last deploy's, for the running labs
  # and a rollback; remove older ones.
  local tags="$bg_state/lab-tags" prev keep
  prev="$({ grep -v "^$short$" "$tags" 2>/dev/null || true; } | tail -n 1)"
  printf '%s\n' ${prev:+"$prev"} "$short" > "$tags.new" && mv -f "$tags.new" "$tags"
  keep=" $short $prev "
  # Best effort: a failed cleanup must never fail a deploy.
  { docker image ls --filter "reference=necropsy-range/$NAME/*" --format '{{.Repository}}:{{.Tag}}' 2>/dev/null || true; } \
    | while read -r ref; do
        case "$keep" in *" ${ref##*:} "*) ;; *) docker rmi "$ref" >/dev/null 2>&1 || true ;; esac
      done || true
  return 0
}

# ---- building a commit ------------------------------------------------------

# The only code ever built is a commit on the configured branch of the
# configured repository,
# so whoever can run this command can choose among releases but not add one.
# Sets $built_image. (Not a command substitution: errors inside one do not stop
# the script.)
built_image=""
build_commit() { # full commit hash
  local sha="$1" build_dir tag
  [[ "$sha" =~ ^[0-9a-f]{40}$ ]] || die "expected a full 40-character commit hash"
  [ -n "$NECROPSY_REPO" ] || die "NECROPSY_REPO is not set in $deploy_conf"
  local branch="$NECROPSY_BRANCH"
  # It ends up in git refspecs, so a plain branch name only.
  [[ "$branch" =~ ^[A-Za-z0-9][A-Za-z0-9._/-]{0,99}$ ]] && [[ "$branch" != *..* ]] \
    && git check-ref-format "refs/heads/$branch" \
    || die "NECROPSY_BRANCH in $deploy_conf is not a valid branch name"
  if [ "$SAFE_MODE" = 1 ]; then
    [[ "$NECROPSY_REPO" =~ ^https://[A-Za-z0-9._/-]+$ ]] || die "NECROPSY_REPO must be an https URL"
    export GIT_ALLOW_PROTOCOL=https
  else
    export GIT_ALLOW_PROTOCOL=file:https
  fi
  export GIT_TERMINAL_PROMPT=0 GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null

  log "fetching $branch from $NECROPSY_REPO"
  if [ ! -d "$src_dir" ]; then
    git clone --quiet --bare --branch "$branch" -- "$NECROPSY_REPO" "$src_dir" >&2
  else
    git -C "$src_dir" remote set-url origin "$NECROPSY_REPO"
    git -C "$src_dir" fetch --quiet --prune origin "+refs/heads/$branch:refs/heads/$branch" >&2
  fi
  git -C "$src_dir" cat-file -e "$sha^{commit}" 2>/dev/null || die "commit $sha is not in the repository"
  git -C "$src_dir" merge-base --is-ancestor "$sha" "refs/heads/$branch" \
    || die "commit $sha is not on $branch, refusing to build it"

  build_dir="$(mktemp -d "$bg_state/build.XXXXXX")"
  # shellcheck disable=SC2064
  trap "rm -rf '$build_dir'" EXIT
  git -C "$src_dir" archive "$sha" | tar -x --no-same-owner -C "$build_dir"
  tag="$NAME:${sha:0:12}"
  log "building $tag"
  docker build --pull -t "$tag" --build-arg "GIT_SHA=$sha" "$build_dir" >&2
  if [ -n "$RANGE_UNIT" ]; then build_range "$build_dir" "$sha"; fi
  rm -rf "$build_dir"
  trap - EXIT
  built_image="$tag"
}

# ---- commands ---------------------------------------------------------------

status() {
  echo "live:    $(active_color || true)"
  echo "blue:    $(image_of blue)"
  echo "green:   $(image_of green)"
  echo "port:    $NECROPSY_BIND:$NECROPSY_PORT"
  if [ -n "$RANGE_UNIT" ]; then
    printf '%-8s %s (%s), at most %s labs\n' "range:" "$RANGE_UNIT" "$(range_unit_active "$RANGE_UNIT")" "$RANGE_MAX_INSTANCES"
  fi
  local c
  for c in "$NAME-blue" "$NAME-green" "$NAME-proxy"; do
    printf '%-16s %s\n' "$c" "$(docker inspect -f '{{.State.Status}} {{if .State.Health}}({{.State.Health.Status}}){{end}}' "$c" 2>/dev/null || echo absent)"
  done
}

command -v docker >/dev/null || die "docker not found"
docker compose version >/dev/null 2>&1 || die "the docker compose plugin is required"
docker info >/dev/null 2>&1 || die "cannot talk to Docker (is the daemon running?)"

mkdir -p "$bg_state" "$proxy_dir"
touch "$images_env"

cmd="${1:-}"
[ -n "$cmd" ] || { usage >&2; exit 2; }
shift

case "$cmd" in
  status)
    [ $# -eq 0 ] || die "status takes no arguments"
    status
    exit 0
    ;;
  deploy|rollback|switch) ;;
  *) die "unknown command: $cmd" ;;
esac

command -v flock >/dev/null || die "flock not found (install util-linux)"
exec 9> "$bg_state/lock"
flock -n 9 || die "another deploy is already running"

case "$cmd" in
  deploy)
    [ $# -eq 1 ] || die "usage: $NAME-deploy deploy <commit>"
    ensure_env_file
    build_commit "$1"
    image="$built_image"
    old="$(active_color)"
    if [ -z "$old" ]; then target=blue; else target="$(other_color "$old")"; fi
    log "deploying $image: ${old:-nothing} -> $target"
    switch_traffic "$target" "$old" "$image"
    prune_images
    ;;

  rollback)
    [ $# -eq 0 ] || die "rollback takes no arguments"
    old="$(active_color)"
    [ -n "$old" ] || die "nothing is deployed yet"
    target="$(other_color "$old")"
    image="$(image_of "$target")"
    [ -n "$image" ] || die "there is no previous release to go back to"
    docker image inspect "$image" >/dev/null 2>&1 || die "the previous image $image is gone"
    log "rolling back from $old to $target"
    switch_traffic "$target" "$old" ""
    ;;

  # Deploys an image that already exists locally. For the test suite only: it
  # would let the caller run any local image, so a real install refuses it.
  switch)
    [ "$SAFE_MODE" != 1 ] || die "unknown command: switch"
    [ $# -eq 1 ] || die "usage: necropsy-deploy switch <image>"
    image="$1"
    valid_image "$image" || die "not a necropsy image: $image"
    docker image inspect "$image" >/dev/null 2>&1 || die "image not found locally: $image"
    ensure_env_file
    old="$(active_color)"
    if [ -z "$old" ]; then target=blue; else target="$(other_color "$old")"; fi
    log "deploying $image: ${old:-nothing} -> $target"
    switch_traffic "$target" "$old" "$image"
    prune_images
    ;;
esac

echo
echo "Logs:    docker logs -f $(container_of "$(active_color)")"
echo "Status:  $NAME-deploy status"
echo "First deploy? The admin credentials were printed once to that log:"
echo "  docker logs $(container_of "$(active_color)") 2>&1 | grep -A4 'first run'"
