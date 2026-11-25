#!/usr/bin/env bash
# Starts every image on a spawner's allowlist, once, and checks that it comes up
# the way the range promises:
#
#   - it answers HTTP on its unix socket
#   - Docker has it with no network, a read-only root, no capabilities, no new
#     privileges, not privileged, an unprivileged user, and hard memory, cpu and
#     process limits
#   - inside, it really has no network interface but loopback, cannot write to
#     its root filesystem, and is not root (checked with what the image has:
#     a check whose tool is missing is skipped, the Docker settings above are
#     the guarantee)
#
# then stops it and checks nothing is left behind.
#
#   selftest.sh <images.toml> <spawner socket> <range dir>
#
# Used by the deploy program (before it switches traffic), by CI against real
# Docker, and by install-vm.sh. Needs root (or the docker group) and curl.

set -euo pipefail

die() { echo "selftest: FAIL: $*" >&2; exit 1; }
ok() { echo "selftest: ok: $*" >&2; }

[ $# -eq 3 ] || { sed -n '2,25p' "$0" | sed 's/^# \{0,1\}//' >&2; exit 2; }
images_toml="$1" socket="$2" range_dir="$3"
[ -f "$images_toml" ] || die "no allowlist at $images_toml"
[ -S "$socket" ] || die "no spawner socket at $socket"

api() { curl -fsS --max-time 90 --unix-socket "$socket" "$@"; }

mapfile -t images < <(sed -n 's/^\[images\.\([a-z0-9-][a-z0-9-]*\)\].*/\1/p' "$images_toml")
[ ${#images[@]} -gt 0 ] || die "the allowlist has no images"

# One image's checks. Runs in a subshell so a failure cleans up and reports.
check_image() {
  local image="$1" id c sock out
  id="st$(od -An -N5 -tx1 /dev/urandom | tr -d ' \n')"
  c="nr-$id"
  sock="$range_dir/$id/app.sock"

  cleanup() { api -X DELETE "http://spawner/v1/instances/$id" >/dev/null 2>&1 || true; }
  trap cleanup EXIT

  api -X POST http://spawner/v1/instances -H 'content-type: application/json' \
    -d "{\"id\":\"$id\",\"image\":\"$image\",\"ttl_secs\":180,\"env\":{\"NECROPSY_FLAG\":\"selftest{ok}\",\"NECROPSY_INSTANCE\":\"$id\"}}" >/dev/null \
    || die "$image: the spawner could not start it (journalctl -u <spawner unit>)"
  [ -S "$sock" ] || die "$image: it never opened its socket"

  # Reachable over the socket. Any HTTP status will do: the point is that it speaks.
  curl -sS --max-time 10 -o /dev/null -w '%{http_code}' --unix-socket "$sock" http://lab/ | grep -q '^[1-5][0-9][0-9]$' \
    || die "$image: it does not answer HTTP on its socket"
  ok "$image answers on its socket"

  # Docker's view of it.
  local f
  f() { docker inspect -f "$1" "$c"; }
  [ "$(f '{{.HostConfig.NetworkMode}}')" = none ] || die "$image: it has a network"
  [ "$(f '{{.HostConfig.ReadonlyRootfs}}')" = true ] || die "$image: the root filesystem is writable"
  [ "$(f '{{.HostConfig.Privileged}}')" = false ] || die "$image: it is privileged"
  f '{{range .HostConfig.CapDrop}}{{.}} {{end}}' | grep -qw ALL || die "$image: capabilities are not dropped"
  f '{{range .HostConfig.SecurityOpt}}{{.}} {{end}}' | grep -q 'no-new-privileges' || die "$image: no-new-privileges is not set"
  [ "$(f '{{.HostConfig.Memory}}')" -gt 0 ] || die "$image: no memory limit"
  [ "$(f '{{.HostConfig.MemorySwap}}')" = "$(f '{{.HostConfig.Memory}}')" ] || die "$image: it can use swap"
  [ "$(f '{{.HostConfig.NanoCpus}}')" -gt 0 ] || die "$image: no cpu limit"
  [ "$(f '{{.HostConfig.PidsLimit}}')" -gt 0 ] || die "$image: no process limit"
  local user
  user="$(f '{{.Config.User}}')"
  case "$user" in ''|0|0:*|root|root:*) die "$image: it runs as root ($user)" ;; esac
  f '{{range .Mounts}}{{.Source}} {{end}}' | grep -q 'docker.sock' && die "$image: it has the Docker socket"
  ok "$image is locked down in Docker's settings"

  # Inside it, with whatever the image has.
  if out="$(docker exec "$c" cat /proc/net/dev 2>/dev/null)"; then
    if awk -F: 'NR > 2 { gsub(/ /, "", $1); if ($1 != "lo") bad = 1 } END { exit !bad }' <<<"$out"; then
      die "$image: it has a network interface besides loopback"
    fi
    ok "$image has no network interface but loopback"
  fi
  if out="$(docker exec "$c" sh -c 'id -u' 2>/dev/null)"; then
    [ "$(tr -d '[:space:]' <<<"$out")" != 0 ] || die "$image: it is running as uid 0"
    ok "$image runs as uid $(tr -d '[:space:]' <<<"$out")"
  fi
  if docker exec "$c" sh -c 'touch /selftest-write 2>/dev/null' 2>/dev/null; then
    die "$image: it could write to its root filesystem"
  fi

  api -X DELETE "http://spawner/v1/instances/$id" >/dev/null
  trap - EXIT
  [ ! -e "$range_dir/$id" ] || die "$image: its socket directory was not removed"
  if docker ps -a --format '{{.Names}}' | grep -qx "$c"; then die "$image: its container was not removed"; fi
}

for image in "${images[@]}"; do
  ( check_image "$image" )
done
echo "selftest: all ${#images[@]} image(s) passed" >&2
