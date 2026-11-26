#!/usr/bin/env bash
# Keeps the range VM's spawner up to date, by itself. Run by a systemd timer
# (necropsy-range-update.timer) as root; it only ever reaches out to the
# repository, so nothing on the range VM needs to be reachable for it.
#
# Each run: fetch the tip of the trusted branch (main), and if it is a new commit
# that changed the spawner (or its unit, or this script), build the spawner from
# it, install it, restart the spawner, and check it is healthy. If it is not, the
# previous program is put back, and that commit is not tried again until a newer
# one arrives. Labs that are running are not affected: the spawner finds them
# again when it starts.
#
# Only the branch named in the settings is ever built, and only fast-forwards
# of what is installed: a rewritten history is refused. The spawner runs as root,
# so the branch must be one that changes only through review. That is the point
# of this being main and not dev.
#
# Settings are read from /etc/necropsy-range/update.env, plain KEY=VALUE lines
# that are parsed, never executed:
#   REPO          https URL of the repository
#   BRANCH        the trusted branch (main)
#   HEALTH_URL    where the spawner answers /healthz, like http://10.0.0.50:9400/healthz
#   SERVICE       the spawner's systemd unit (necropsy-spawner)
#
# Run by hand with `systemctl start necropsy-range-update` to check now.

set -euo pipefail
shopt -s inherit_errexit

# Paths and the settings file can be moved only for the test suite, through the
# environment, which a systemd service does not have.
CONF="${NECROPSY_UPDATE_CONF:-/etc/necropsy-range/update.env}"
TESTING="${NECROPSY_UPDATE_TESTING:-0}"

die() { echo "range-update: error: $*" >&2; exit 1; }
log() { echo "range-update: $*" >&2; }

REPO="" BRANCH=main HEALTH_URL="" SERVICE=necropsy-spawner
SPAWNER_BIN=/usr/local/bin/necropsy-spawner
STATE_DIR=/var/lib/necropsy-range/updater
UNIT_DIR=/etc/systemd/system
LIB_DIR=/usr/local/lib/necropsy-range

[ -f "$CONF" ] || die "no settings at $CONF"
if [ "$TESTING" != 1 ]; then
  # A file anyone else can write is a way to change what runs as root.
  [ "$(stat -c %u "$CONF")" = 0 ] && [ $(( 8#$(stat -c %a "$CONF") & 8#022 )) -eq 0 ] \
    || die "$CONF must be owned by root and not writable by others"
fi
while IFS='=' read -r key value || [ -n "$key" ]; do
  case "$key" in ''|'#'*) continue ;; esac
  value="${value%\"}"; value="${value#\"}"
  case "$key" in
    REPO|BRANCH|HEALTH_URL|SERVICE) printf -v "$key" '%s' "$value" ;;
    # Where things are: moved only by the test suite.
    SPAWNER_BIN|STATE_DIR|UNIT_DIR|LIB_DIR) [ "$TESTING" = 1 ] && printf -v "$key" '%s' "$value" ;;
  esac
done < "$CONF"

if [ "$TESTING" = 1 ]; then
  [[ "$REPO" =~ ^(https|file)://[A-Za-z0-9._/:-]+$ ]] || die "REPO must be an https URL"
else
  [[ "$REPO" =~ ^https://[A-Za-z0-9._/-]+$ ]] || die "REPO must be an https URL"
fi
[[ "$BRANCH" =~ ^[A-Za-z0-9][A-Za-z0-9._/-]{0,99}$ ]] && [[ "$BRANCH" != *..* ]] || die "BRANCH is not a plain branch name"
[[ "$HEALTH_URL" =~ ^http://[A-Za-z0-9.-]+:[0-9]{1,5}/healthz$ ]] || die "HEALTH_URL must look like http://10.0.0.50:9400/healthz"
[[ "$SERVICE" =~ ^[a-z][a-z0-9-]{0,40}$ ]] || die "SERVICE is not a plain unit name"

mkdir -p "$STATE_DIR"
exec 9> "$STATE_DIR/lock"
flock -n 9 || { log "another update is running"; exit 0; }

if [ "$TESTING" = 1 ]; then
  export GIT_ALLOW_PROTOCOL=file:https
else
  export GIT_ALLOW_PROTOCOL=https
fi
export GIT_TERMINAL_PROMPT=0 GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null

src="$STATE_DIR/src.git"
if [ ! -d "$src" ]; then
  git clone --quiet --bare --branch "$BRANCH" -- "$REPO" "$src"
else
  git -C "$src" remote set-url origin "$REPO"
  git -C "$src" fetch --quiet --prune origin "+refs/heads/$BRANCH:refs/heads/$BRANCH"
fi
new="$(git -C "$src" rev-parse "refs/heads/$BRANCH")"
[[ "$new" =~ ^[0-9a-f]{40}$ ]] || die "could not read the tip of $BRANCH"

installed="$(cat "$STATE_DIR/installed" 2>/dev/null || true)"
failed="$(cat "$STATE_DIR/failed" 2>/dev/null || true)"
[ "$new" != "$installed" ] || exit 0
if [ "$new" = "$failed" ]; then
  log "${new:0:12} failed before; waiting for a newer commit on $BRANCH"
  exit 0
fi
# A branch that has no spawner yet (main, before the change that adds it is
# merged) has nothing to build. Leave what is installed alone, and look again next
# time; this is not a failure.
if ! git -C "$src" cat-file -e "$new:crates/necropsy-spawner/Cargo.toml" 2>/dev/null; then
  log "${new:0:12} on $BRANCH has no spawner in it (yet), so the installed one is left alone"
  exit 0
fi

if [ -n "$installed" ] && ! git -C "$src" merge-base --is-ancestor "$installed" "$new"; then
  echo "$new" > "$STATE_DIR/failed"
  die "$BRANCH is no longer a descendant of what is installed (${installed:0:12}): history was rewritten. Not updating. Fix it by hand, then delete $STATE_DIR/installed"
fi

# Only a commit that touches the spawner, its unit, or this script needs work.
relevant=(crates/necropsy-spawner Cargo.toml Cargo.lock Dockerfile deploy/range)
if [ -n "$installed" ] && git -C "$src" diff --quiet "$installed" "$new" -- "${relevant[@]}"; then
  log "${new:0:12}: nothing in it changes the spawner"
  echo "$new" > "$STATE_DIR/installed"
  exit 0
fi

build="$(mktemp -d "$STATE_DIR/build.XXXXXX")"
out="$(mktemp -d "$STATE_DIR/out.XXXXXX")"
trap 'rm -rf "$build" "$out"' EXIT
git -C "$src" archive "$new" | tar -x --no-same-owner -C "$build"

fail() { # message: remember this commit failed, so it is not retried every run
  echo "$new" > "$STATE_DIR/failed"
  die "${new:0:12}: $1"
}

rust="$(sed -n 's/^ARG RUST_VERSION=//p' "$build/Dockerfile" | head -n 1)"
[[ "$rust" =~ ^[0-9][0-9.]*$ ]] || fail "cannot find the Rust version in the Dockerfile"
log "${new:0:12}: building the spawner"
docker run --rm \
  -v "$build":/src:ro \
  -v necropsy-build-cargo:/usr/local/cargo/registry \
  -v necropsy-build-target:/target \
  -v "$out":/out \
  -e CARGO_TARGET_DIR=/target -w /src \
  "rust:$rust-alpine" \
  sh -c 'apk add --no-cache musl-dev >/dev/null && cargo build --release --locked -p necropsy-spawner && cp /target/release/necropsy-spawner /out/' >&2 \
  || fail "the spawner did not build"
[ -s "$out/necropsy-spawner" ] || fail "the spawner did not build"

changed=0
if ! cmp -s "$out/necropsy-spawner" "$SPAWNER_BIN"; then
  [ ! -f "$SPAWNER_BIN" ] || cp -p "$SPAWNER_BIN" "$SPAWNER_BIN.prev"
  install -m 755 "$out/necropsy-spawner" "$SPAWNER_BIN.new"
  mv -f "$SPAWNER_BIN.new" "$SPAWNER_BIN"
  changed=1
fi
unit_src="$build/deploy/range/necropsy-spawner.service"
if [ -f "$unit_src" ] && ! cmp -s "$unit_src" "$UNIT_DIR/$SERVICE.service"; then
  [ ! -f "$UNIT_DIR/$SERVICE.service" ] || cp -p "$UNIT_DIR/$SERVICE.service" "$STATE_DIR/unit.prev"
  install -m 644 "$unit_src" "$UNIT_DIR/$SERVICE.service"
  systemctl daemon-reload
  changed=1
fi

if [ "$changed" = 1 ]; then
  log "${new:0:12}: restarting $SERVICE"
  healthy=0
  if systemctl restart "$SERVICE"; then
    for _ in $(seq 1 30); do
      if curl -fsS --max-time 2 "$HEALTH_URL" >/dev/null 2>&1; then healthy=1; break; fi
      sleep 1
    done
  fi
  if [ "$healthy" != 1 ]; then
    log "${new:0:12}: the new spawner is not healthy, putting the previous one back"
    [ ! -f "$SPAWNER_BIN.prev" ] || mv -f "$SPAWNER_BIN.prev" "$SPAWNER_BIN"
    if [ -f "$STATE_DIR/unit.prev" ]; then
      mv -f "$STATE_DIR/unit.prev" "$UNIT_DIR/$SERVICE.service"
      systemctl daemon-reload
    fi
    systemctl restart "$SERVICE" || true
    fail "the new spawner did not become healthy, so the previous one was restored"
  fi
fi
rm -f "$SPAWNER_BIN.prev" "$STATE_DIR/unit.prev"

# This script too, from the same trusted commit; it takes effect on the next run.
if [ -f "$build/deploy/range/update.sh" ] && ! cmp -s "$build/deploy/range/update.sh" "$LIB_DIR/update.sh"; then
  install -d -m 755 "$LIB_DIR"
  install -m 755 "$build/deploy/range/update.sh" "$LIB_DIR/update.sh.new"
  mv -f "$LIB_DIR/update.sh.new" "$LIB_DIR/update.sh"
fi

echo "$new" > "$STATE_DIR/installed"
rm -f "$STATE_DIR/failed"
log "${new:0:12}: installed (the spawner is $([ "$changed" = 1 ] && echo updated || echo unchanged))"
