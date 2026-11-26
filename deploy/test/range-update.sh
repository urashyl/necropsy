#!/usr/bin/env bash
# Tests deploy/range/update.sh, the range VM's self-update, against a local git
# repository and fake docker, systemctl and curl, so nothing is installed.
#
#   deploy/test/range-update.sh

set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$here/../range/update.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
failures=0

check() { # description, command...
  local what="$1" out; shift
  if out="$("$@" 2>&1)"; then
    echo "ok   $what"
  else
    echo "FAIL $what"
    [ -z "$out" ] || printf '%s\n' "$out" | head -8 | sed 's/^/       | /'
    failures=$((failures + 1))
  fi
}
equals() { [ "$1" = "$2" ]; }
fails() { ! "$@"; }

# ---- a repository, an install, and fakes -------------------------------------------

origin="$work/origin.git"; src="$work/src"
git init -q --bare -b main "$origin"
git init -q -b main "$src"
git -C "$src" config user.email t@t; git -C "$src" config user.name t
mkdir -p "$src/crates/necropsy-spawner" "$src/deploy/range" "$src/docs"
printf 'ARG RUST_VERSION=1.100\nFROM scratch\n' > "$src/Dockerfile"
echo '[workspace]' > "$src/Cargo.toml"; echo '# lock' > "$src/Cargo.lock"
echo v1 > "$src/crates/necropsy-spawner/marker.txt"
echo unit-v1 > "$src/deploy/range/necropsy-spawner.service"
echo updater-v1 > "$src/deploy/range/update.sh"
echo docs-v1 > "$src/docs/readme.txt"
git -C "$src" add -A; git -C "$src" commit -q -m one
git -C "$src" remote add origin "$origin"; git -C "$src" push -q origin main

fakebin="$work/fakebin"; cp -r "$here/rangeupdate-fakes" "$fakebin"
export FAKE_DIR="$work/fake"; mkdir -p "$FAKE_DIR"; echo "$FAKE_DIR" > "$fakebin/state_dir"
mkdir -p "$work/bin" "$work/units" "$work/lib" "$work/state"
echo "$work/bin/necropsy-spawner" > "$FAKE_DIR/bin_path"
echo old-binary > "$work/bin/necropsy-spawner"; echo old-unit > "$work/units/necropsy-spawner.service"

conf="$work/update.env"
cat > "$conf" <<ENV
REPO=file://$origin
BRANCH=main
HEALTH_URL=http://10.0.0.50:9400/healthz
SERVICE=necropsy-spawner
SPAWNER_BIN=$work/bin/necropsy-spawner
STATE_DIR=$work/state
UNIT_DIR=$work/units
LIB_DIR=$work/lib
ENV

update() { PATH="$fakebin:$PATH" NECROPSY_UPDATE_CONF="$conf" NECROPSY_UPDATE_TESTING=1 "$script" > "$work/out" 2>&1; }
calls() { cat "$FAKE_DIR/calls" 2>/dev/null; }
n() { calls | grep -c "$1" || true; }
mark() { calls | wc -l; }
since() { calls | tail -n +"$(( $1 + 1 ))"; }
out_has() { grep -q -- "$1" "$work/out"; }
sha_of() { git -C "$src" rev-parse "$1"; }
push() { git -C "$src" add -A; git -C "$src" commit -q -m "$1"; git -C "$src" push -q origin "${2:-main}"; }
installed() { cat "$work/state/installed" 2>/dev/null; }

# ---- the first run -----------------------------------------------------------------

update
check "the first run succeeds"                       test $? -eq 0
check "it built the spawner"                         equals "$(n 'docker run')" 1
check "and installed it"                             equals "$(cat "$work/bin/necropsy-spawner")" v1
check "...executable"                                test -x "$work/bin/necropsy-spawner"
check "and the unit"                                 equals "$(cat "$work/units/necropsy-spawner.service")" unit-v1
check "it reloaded systemd and restarted the spawner" bash -c "grep -q 'systemctl daemon-reload' '$FAKE_DIR/calls' && grep -q 'systemctl restart necropsy-spawner' '$FAKE_DIR/calls'"
check "it checked the spawner was healthy"           bash -c "grep -q 'curl .*http://10.0.0.50:9400/healthz' '$FAKE_DIR/calls'"
check "it installed this updater from the commit"    equals "$(cat "$work/lib/update.sh")" updater-v1
check "it recorded what is installed"                equals "$(installed)" "$(sha_of HEAD)"
check "the build ran against the trusted tree, with caches" bash -c "grep -q 'necropsy-build-cargo' '$FAKE_DIR/calls' && grep -q 'rust:1.100-alpine' '$FAKE_DIR/calls'"
check "no build directory is left behind"            bash -c "! ls '$work'/state/build.* '$work'/state/out.* >/dev/null 2>&1"
check "the previous program is not kept once it works" test ! -e "$work/bin/necropsy-spawner.prev"

# ---- nothing new -------------------------------------------------------------------

m="$(mark)"; update
check "a run with nothing new does nothing"          equals "$(since "$m" | grep -c 'docker\|systemctl')" 0

# ---- commits that do not touch the spawner ------------------------------------------

echo docs-v2 > "$src/docs/readme.txt"; push "docs only"
m="$(mark)"; update
check "a docs-only commit builds nothing"            equals "$(since "$m" | grep -c 'docker run')" 0
check "...and restarts nothing"                      equals "$(since "$m" | grep -c 'systemctl restart')" 0
check "...but is recorded as seen"                   equals "$(installed)" "$(sha_of HEAD)"
check "...and says why"                              out_has "changes the spawner"

# ---- a spawner change --------------------------------------------------------------

echo v2 > "$src/crates/necropsy-spawner/marker.txt"; push "spawner v2"
m="$(mark)"; update
check "a spawner change is built"                    equals "$(since "$m" | grep -c 'docker run')" 1
check "...installed"                                 equals "$(cat "$work/bin/necropsy-spawner")" v2
check "...and the spawner restarted"                 equals "$(since "$m" | grep -c 'systemctl restart necropsy-spawner')" 1
check "...recorded"                                  equals "$(installed)" "$(sha_of HEAD)"
check "a rebuild that gives the same program restarts nothing" bash -c "
  echo comment >> '$src/Cargo.lock'; cd '$src' && git add -A && git commit -q -m lock && git push -q origin main
  m=\$(wc -l < '$FAKE_DIR/calls'); PATH='$fakebin:$PATH' NECROPSY_UPDATE_CONF='$conf' NECROPSY_UPDATE_TESTING=1 '$script' >/dev/null 2>&1
  [ \"\$(tail -n +\$((m + 1)) '$FAKE_DIR/calls' | grep -c 'systemctl restart')\" = 0 ]"
check "...but it is recorded"                        equals "$(installed)" "$(sha_of HEAD)"

# ---- the unit and the updater itself ------------------------------------------------

echo unit-v2 > "$src/deploy/range/necropsy-spawner.service"; echo updater-v2 > "$src/deploy/range/update.sh"; push "unit and updater"
m="$(mark)"; update
check "a changed unit is installed"                  equals "$(cat "$work/units/necropsy-spawner.service")" unit-v2
check "...with a reload and a restart"               bash -c "tail -n +$((m + 1)) '$FAKE_DIR/calls' | grep -q 'systemctl daemon-reload' && tail -n +$((m + 1)) '$FAKE_DIR/calls' | grep -q 'systemctl restart'"
check "a changed updater replaces itself"            equals "$(cat "$work/lib/update.sh")" updater-v2

# ---- failures ----------------------------------------------------------------------

echo v3 > "$src/crates/necropsy-spawner/marker.txt"; push "spawner v3, will not build"
sha_build="$(sha_of HEAD)"
: > "$FAKE_DIR/build_fail"
m="$(mark)"; update
check "a build that fails fails the run"             test $? -ne 0
check "...and says so"                               out_has "did not build"
check "...and installs nothing"                      equals "$(cat "$work/bin/necropsy-spawner")" v2
check "...and restarts nothing"                      equals "$(since "$m" | grep -c 'systemctl restart')" 0
check "...and the installed commit is unchanged"     bash -c "[ \"\$(cat '$work/state/installed')\" != '$sha_build' ]"
check "...and that commit is remembered as failed"   equals "$(cat "$work/state/failed")" "$sha_build"
m="$(mark)"; update
check "the same commit is not tried again"           equals "$(since "$m" | grep -c 'docker run')" 0
check "...and says it is waiting"                    out_has "waiting for a newer commit"
rm -f "$FAKE_DIR/build_fail"
echo v4 > "$src/crates/necropsy-spawner/marker.txt"; push "spawner v4"
update
check "a newer commit is tried, and works"           equals "$(cat "$work/bin/necropsy-spawner")" v4
check "...and clears the failure"                    test ! -e "$work/state/failed"

echo BAD > "$src/crates/necropsy-spawner/marker.txt"; push "spawner that starts unhealthy"
sha_bad="$(sha_of HEAD)"
m="$(mark)"; update
check "an unhealthy new spawner fails the run"       test $? -ne 0
check "...and the previous program is put back"      equals "$(cat "$work/bin/necropsy-spawner")" v4
check "...and the spawner is restarted on it"        equals "$(since "$m" | grep -c 'systemctl restart necropsy-spawner')" 2
check "...and that commit is remembered as failed"   equals "$(cat "$work/state/failed")" "$sha_bad"
check "...and what is installed is still the old commit" bash -c "[ \"\$(cat '$work/state/installed')\" != '$sha_bad' ]"
check "...and says why"                              out_has "previous one was restored"

: > "$FAKE_DIR/restart_fail"
echo v5 > "$src/crates/necropsy-spawner/marker.txt"; push "spawner v5"
update
check "a spawner that cannot restart fails the run"  test $? -ne 0
check "...and the previous program is back"          equals "$(cat "$work/bin/necropsy-spawner")" v4
rm -f "$FAKE_DIR/restart_fail"
echo v6 > "$src/crates/necropsy-spawner/marker.txt"; push "spawner v6"
update
check "and the next good commit installs"            equals "$(cat "$work/bin/necropsy-spawner")" v6

# ---- only the trusted branch, only forward -------------------------------------------

before="$(installed)"
git -C "$src" checkout -q -b dev; echo v-dev > "$src/crates/necropsy-spawner/marker.txt"; push "on dev" dev
git -C "$src" checkout -q main
m="$(mark)"; update
check "a commit on another branch is ignored"        equals "$(since "$m" | grep -c 'docker run')" 0
check "...and nothing changed"                       equals "$(installed)" "$before"

git -C "$src" reset -q --hard HEAD~2
echo rewritten > "$src/crates/necropsy-spawner/marker.txt"; git -C "$src" add -A; git -C "$src" commit -q -m rewritten
git -C "$src" push -q -f origin main
m="$(mark)"; update
check "a rewritten history is refused"               test $? -ne 0
check "...with the reason"                           out_has "history was rewritten"
check "...and nothing is built or restarted"         equals "$(since "$m" | grep -c 'docker run\|systemctl restart')" 0
check "...and the program is untouched"              equals "$(cat "$work/bin/necropsy-spawner")" v6

# ---- settings are checked ------------------------------------------------------------

try_conf() { # replacement line(s), key to replace
  cp "$conf" "$work/update.env.bak"
  sed -i "/^$2=/d" "$conf"; printf '%s\n' "$1" >> "$conf"
  update; local rc=$?
  cp "$work/update.env.bak" "$conf"; return $rc
}
check "a branch that is not plain is refused"        fails try_conf 'BRANCH=../x' BRANCH
check "a branch that looks like an option is refused" fails try_conf 'BRANCH=-x' BRANCH
check "a health url with a path is refused"          fails try_conf 'HEALTH_URL=http://10.0.0.50:9400/healthz/../x' HEALTH_URL
check "a health url that is not http is refused"     fails try_conf 'HEALTH_URL=ftp://x:1/healthz' HEALTH_URL
check "a unit name that is not plain is refused"     fails try_conf 'SERVICE=a b' SERVICE
check "a repository that is not a URL is refused"    fails try_conf 'REPO=/etc' REPO
rm -f "$work/state/failed"
check "a missing settings file is refused"           bash -c "PATH='$fakebin:$PATH' NECROPSY_UPDATE_CONF='$work/nope' NECROPSY_UPDATE_TESTING=1 '$script' >/dev/null 2>&1; [ \$? -ne 0 ]"

# Without the test switch, the paths cannot be moved and the repository must be https.
check "outside the test switch a file:// repository is refused" bash -c "
  PATH='$fakebin:$PATH' NECROPSY_UPDATE_CONF='$conf' '$script' 2>&1 | grep -q 'must be owned by root\|https URL'"

echo
if [ "$failures" -eq 0 ]; then echo "all passed"; else echo "$failures failed"; exit 1; fi
