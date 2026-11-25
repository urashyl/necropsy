#!/usr/bin/env bash
# Tests the range against real Docker, the way it runs in production: starts the
# real spawner with two tenants, builds the labs, pushes their images over the API
# as a site's deploy does, sends the allowlist, and checks:
#
#   - the spawner's own self-test passes for every lab (each comes up with no
#     network, a read-only root, no capabilities, limits; answers; is removed)
#   - each lab's own test (range/<lab>/test.sh) passes, through the lab proxy
#   - an expired lab is reaped, and a full range refuses another
#   - tenants are separate: one cannot reach, list, stop or push over another's
#     labs, and an image that claims another tenant's name is refused
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
  docker image ls --filter 'reference=necropsy-range/ci*/*' --format '{{.Repository}}:{{.Tag}}' | xargs -r docker rmi >/dev/null 2>&1 || true
  rm -rf "$tmp"
}
trap cleanup EXIT

tok_a="ci-token-a-0123456789abcdef0123456789abcdef"
tok_b="ci-token-b-0123456789abcdef0123456789abcdef"
hash() { printf '%s' "$1" | sha256sum | cut -d' ' -f1; }
cat > "$tmp/tenants.toml" <<TOML
[tenants.cia]
token_sha256 = "$(hash "$tok_a")"
max_instances = 4
memory_budget_mb = 1024

[tenants.cib]
token_sha256 = "$(hash "$tok_b")"
max_instances = 2
memory_budget_mb = 512
TOML

# A free port.
port="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')"
base="http://127.0.0.1:$port"
mkdir -p "$tmp/state" "$tmp/range"
SPAWNER_LISTEN="127.0.0.1:$port" SPAWNER_TENANTS="$tmp/tenants.toml" SPAWNER_STATE="$tmp/state" \
  SPAWNER_RANGE_DIR="$tmp/range" "$spawner" > "$tmp/spawner.log" 2>&1 &
spawner_pid=$!
for _ in $(seq 1 50); do curl -fsS "$base/healthz" >/dev/null 2>&1 && break; sleep 0.2; done
curl -fsS "$base/healthz" >/dev/null 2>&1 || { cat "$tmp/spawner.log" >&2; die "the spawner did not start"; }

api() { local t="$1"; shift; curl -fsS --max-time 120 -H "X-Necropsy-Token: $t" "$@"; }
code() { local t="$1"; shift; curl -sS -o /dev/null -w '%{http_code}' --max-time 120 -H "X-Necropsy-Token: $t" "$@"; }

[ "$(code wrong-token-0123456789abcdef "$base/v1/instances")" = 401 ] || die "a wrong token was accepted"
[ "$(code "" "$base/v1/instances")" = 401 ] || die "no token was accepted"

# Build each lab, push its image, and send the allowlist, as the deploy does.
refs=()
for dir in "$repo_root"/range/*/; do
  [ -f "$dir/Dockerfile" ] || continue
  lab="$(basename "$dir")"
  info "building lab $lab"
  docker build -q -t "necropsy-range/cia/$lab:ci" "$dir" >/dev/null
  docker save -o "$tmp/$lab.tar" "necropsy-range/cia/$lab:ci"
  info "pushing $lab"
  api "$tok_a" -X POST -T "$tmp/$lab.tar" "$base/v1/images" >/dev/null
  refs+=("$lab")
done
sed -e 's|"necropsy-range/\([a-z0-9-]*\):1"|"necropsy-range/cia/\1:ci"|' "$repo_root/range/images.toml" > "$tmp/allowlist.toml"
api "$tok_a" -X PUT --data-binary "@$tmp/allowlist.toml" "$base/v1/allowlist" >/dev/null

info "the spawner's own test, over every lab"
api "$tok_a" -X POST "$base/v1/selftest" >&2 || { api "$tok_a" -X POST "$base/v1/selftest" >&2 || true; die "the spawner's self-test failed"; }

for lab in "${refs[@]}"; do
  t="$repo_root/range/$lab/test.sh"
  [ -f "$t" ] || continue
  info "the $lab lab's own test"
  "$t" "$base" "$tok_a" "$lab"
done

info "tenants are separate"
first="${refs[0]}"
api "$tok_a" -X POST "$base/v1/instances" -H 'content-type: application/json' \
  -d "{\"id\":\"iso0000001\",\"image\":\"$first\",\"ttl_secs\":120}" >/dev/null || die "could not start a lab"
[ "$(code "$tok_b" "$base/v1/instances")" = 200 ] || die "the other tenant cannot list"
api "$tok_b" "$base/v1/instances" | grep -q iso0000001 && die "a tenant sees another's lab"
[ "$(code "$tok_b" -H 'X-Necropsy-Instance: iso0000001' "$base/")" = 404 ] || die "a tenant reached another's lab through the proxy"
[ "$(code "$tok_b" -X POST -H 'content-type: application/json' -d '{"ttl_secs":5}' "$base/v1/instances/iso0000001/touch")" = 404 ] || die "a tenant extended another's lab"
api "$tok_b" -X DELETE "$base/v1/instances/iso0000001" >/dev/null
api "$tok_a" "$base/v1/instances" | grep -q iso0000001 || die "a tenant stopped another's lab"
# The other tenant has no allowlist, so it can start nothing.
[ "$(code "$tok_b" -X POST -H 'content-type: application/json' -d "{\"id\":\"iso0000002\",\"image\":\"$first\",\"ttl_secs\":60}" "$base/v1/instances")" = 400 ] || die "a tenant with no allowlist started a lab"
# An image that claims the first tenant's name, pushed by the second.
[ "$(code "$tok_b" -X POST -T "$tmp/$first.tar" "$base/v1/images")" = 400 ] || die "a tenant pushed an image under another's name"
api "$tok_a" -X DELETE "$base/v1/instances/iso0000001" >/dev/null

info "an expired lab is reaped"
api "$tok_a" -X POST "$base/v1/instances" -H 'content-type: application/json' \
  -d "{\"id\":\"reapme0001\",\"image\":\"$first\",\"ttl_secs\":1}" >/dev/null
for _ in $(seq 1 40); do
  docker ps -a --format '{{.Names}}' | grep -qx nr-cia-reapme0001 || break
  sleep 1
done
if docker ps -a --format '{{.Names}}' | grep -qx nr-cia-reapme0001; then die "an expired lab was not reaped"; fi
[ ! -e "$tmp/range/cia/reapme0001" ] || die "an expired lab's directory was not removed"

info "the range is full when it should be"
for i in 1 2 3 4; do
  api "$tok_a" -X POST "$base/v1/instances" -H 'content-type: application/json' \
    -d "{\"id\":\"full000$i\",\"image\":\"$first\",\"ttl_secs\":120}" >/dev/null || die "could not start lab $i of 4"
done
[ "$(code "$tok_a" -X POST -H 'content-type: application/json' -d "{\"id\":\"full0005\",\"image\":\"$first\",\"ttl_secs\":120}" "$base/v1/instances")" = 429 ] \
  || die "a fifth lab was accepted for a tenant limited to four"

info "all good"
