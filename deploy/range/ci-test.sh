#!/usr/bin/env bash
# Tests the range against real Docker, the way it runs in production: builds the
# lab images, starts the real spawner with an allowlist of them, runs the
# generic self-test (deploy/range/selftest.sh) over every image, then each lab's
# own test (range/<lab>/test.sh), and checks an expired lab is reaped.
#
#   sudo env "PATH=$PATH" deploy/range/ci-test.sh [path to the spawner program]
#
# Needs root (the spawner hands each lab's directory to the lab's user), Docker,
# and curl. The spawner program defaults to target/release/necropsy-spawner:
# build it first with `cargo build --release -p necropsy-spawner`. Everything it
# creates is removed afterwards.

set -euo pipefail

die() { echo "range ci: FAIL: $*" >&2; exit 1; }
info() { echo "range ci: $*" >&2; }

[ "$(id -u)" -eq 0 ] || die "run this as root"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
spawner="${1:-$repo_root/target/release/necropsy-spawner}"
[ -x "$spawner" ] || die "no spawner program at $spawner (cargo build --release -p necropsy-spawner)"

tmp="$(mktemp -d)"
spawner_pid=""
cleanup() {
  [ -z "$spawner_pid" ] || kill "$spawner_pid" 2>/dev/null || true
  docker ps -aq --filter label=necropsy.range=1 | xargs -r docker rm -f >/dev/null 2>&1 || true
  docker image ls --filter 'reference=necropsy-range/ci/*' --format '{{.Repository}}:{{.Tag}}' | xargs -r docker rmi >/dev/null 2>&1 || true
  rm -rf "$tmp"
}
trap cleanup EXIT

# The labs, tagged so they cannot be mistaken for a real deploy's.
for dir in "$repo_root"/range/*/; do
  [ -f "$dir/Dockerfile" ] || continue
  lab="$(basename "$dir")"
  info "building lab $lab"
  docker build -q -t "necropsy-range/ci/$lab:test" "$dir" >/dev/null
done

sed -e 's|"necropsy-range/\([a-z0-9-]*\):1"|"necropsy-range/ci/\1:test"|' \
    -e 's|^max_instances *=.*|max_instances = 4|' \
    "$repo_root/range/images.toml" > "$tmp/images.toml"

mkdir -p "$tmp/range"
SPAWNER_IMAGES="$tmp/images.toml" SPAWNER_SOCKET="$tmp/spawner.sock" SPAWNER_RANGE_DIR="$tmp/range" \
  SPAWNER_SOCKET_GID="$(id -g)" "$spawner" > "$tmp/spawner.log" 2>&1 &
spawner_pid=$!
for _ in $(seq 1 50); do [ -S "$tmp/spawner.sock" ] && break; sleep 0.2; done
[ -S "$tmp/spawner.sock" ] || { cat "$tmp/spawner.log" >&2; die "the spawner did not start"; }
api() { curl -fsS --unix-socket "$tmp/spawner.sock" "$@"; }
api http://spawner/healthz >/dev/null || die "the spawner does not answer"

info "the generic self-test, over every image"
"$repo_root/deploy/range/selftest.sh" "$tmp/images.toml" "$tmp/spawner.sock" "$tmp/range"

for t in "$repo_root"/range/*/test.sh; do
  [ -f "$t" ] || continue
  lab="$(basename "$(dirname "$t")")"
  info "the $lab lab's own test"
  "$t" "$tmp/spawner.sock" "$tmp/range" "$lab"
done

info "an expired lab is reaped"
first="$(sed -n 's/^\[images\.\([a-z0-9-]*\)\].*/\1/p' "$tmp/images.toml" | head -n 1)"
api -X POST http://spawner/v1/instances -H 'content-type: application/json' \
  -d "{\"id\":\"reapme0001\",\"image\":\"$first\",\"ttl_secs\":1,\"env\":{\"NECROPSY_FLAG\":\"x{y}\"}}" >/dev/null
for _ in $(seq 1 40); do
  docker ps -a --format '{{.Names}}' | grep -qx nr-reapme0001 || break
  sleep 1
done
if docker ps -a --format '{{.Names}}' | grep -qx nr-reapme0001; then die "an expired lab was not reaped"; fi
[ ! -e "$tmp/range/reapme0001" ] || die "an expired lab's directory was not removed"

info "the range is full when it should be"
for i in 1 2 3 4; do
  api -X POST http://spawner/v1/instances -H 'content-type: application/json' \
    -d "{\"id\":\"full000$i\",\"image\":\"$first\",\"ttl_secs\":120}" >/dev/null || die "could not start lab $i of 4"
done
code="$(curl -sS -o /dev/null -w '%{http_code}' --unix-socket "$tmp/spawner.sock" -X POST http://spawner/v1/instances \
  -H 'content-type: application/json' -d "{\"id\":\"full0005\",\"image\":\"$first\",\"ttl_secs\":120}")"
[ "$code" = 429 ] || die "a fifth lab was accepted on a range of four (HTTP $code)"

info "all good"
