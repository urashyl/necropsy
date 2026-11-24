#!/usr/bin/env bash
# Tests the deploy program and its installer against a fake docker (see
# fakebin/docker), so no Docker daemon is needed:
#
#   deploy/bluegreen/test/run.sh          the switching, git and installer tests
#   sudo env PATH="$PATH" deploy/bluegreen/test/run.sh
#                                         also the tests of the real (safe) mode,
#                                         which need root

set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
installer="$here/../install-system.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
# A private copy, so it can point at this run's state without the environment.
fakebin="$work/fakebin"
cp -r "$here/fakebin" "$fakebin"

export FAKE_DIR="$work/fake"
export HOME="$work/home"
mkdir -p "$HOME" "$FAKE_DIR/images"
echo "$FAKE_DIR" > "$fakebin/state_dir"
unset XDG_CONFIG_HOME XDG_STATE_HOME DOCKER_HOST
failures=0

check() { # description, command...
  local what="$1"; shift
  if "$@" >/dev/null 2>&1; then echo "ok   $what"; else echo "FAIL $what"; failures=$((failures + 1)); fi
}
equals() { [ "$1" = "$2" ]; }
fails() { ! "$@"; }
skipped() { echo "skip $1"; }

# ---- an install in a temporary root ------------------------------------------

# $1 = name, rest = extra installer options. Prints the install root.
install_at() {
  local root="$work/$1"; shift
  "$installer" --user nobody --repo "file://$work/origin.git" \
    --prefix "$root/usr" --sysconfdir "$root/etc" --statedir "$root/var" \
    --sudoers-dir "$root/sudoers.d" --extra-path "$fakebin" "$@" > "$root.log" 2>&1
}

root="$work/t"
install_at t --unsafe-for-tests
bin="$root/usr/bin/necropsy-deploy"
state="$root/var/necropsy/bluegreen"
conf="$root/etc/necropsy"

# A repository to build from: two commits on main, one commit only on a branch.
origin="$work/origin.git"
src="$work/src"
git init -q --bare -b main "$origin"
git init -q -b main "$src"
git -C "$src" config user.email t@t
git -C "$src" config user.name t
echo 'FROM scratch' > "$src/Dockerfile"; echo one > "$src/file.txt"
git -C "$src" add -A; git -C "$src" commit -q -m one
sha_a="$(git -C "$src" rev-parse HEAD)"
git -C "$src" checkout -q -b side
echo side > "$src/side.txt"; git -C "$src" add -A; git -C "$src" commit -q -m side
sha_side="$(git -C "$src" rev-parse HEAD)"
git -C "$src" checkout -q main
git -C "$src" remote add origin "$origin"
git -C "$src" push -q origin main side

PATH="$fakebin:$PATH"
export DRAIN_SECONDS=0   # ignored by the program; settings come from deploy.env
printf 'DRAIN_SECONDS=0\nHEALTH_TIMEOUT=3\n' >> "$conf/deploy.env"

run() { "$bin" "$@" > "$work/out" 2>&1; }
add_image() { : > "$FAKE_DIR/images/$1"; }
release() { add_image "$1"; run switch "$1"; }
active() { cat "$state/active" 2>/dev/null; }
upstream() { cat "$state/proxy/upstream.caddy" 2>/dev/null; }
running() { [ "$(cat "$FAKE_DIR/c/$1/running" 2>/dev/null)" = true ]; }
image_in() { cat "$FAKE_DIR/c/$1/image" 2>/dev/null; }
img_exists() { [ -e "$FAKE_DIR/images/$1" ]; }
out_has() { grep -q -- "$1" "$work/out"; }

echo "# installer"
check "program is installed, executable by all, writable only by its owner" test "$(stat -c %a "$bin")" = 755
check "compose file is installed" test -f "$root/usr/lib/necropsy/bluegreen/compose.yaml"
check "Caddyfile is installed" test -f "$root/usr/lib/necropsy/bluegreen/Caddyfile"
check "settings file is private" test "$(stat -c %a "$conf/necropsy.env")" = 600
check "sudo rule is read-only" test "$(stat -c %a "$root/sudoers.d/necropsy-deploy")" = 440
check "program has its paths baked in" grep -q "STATEDIR='$root/var'" "$bin"
check "the template placeholders are gone" bash -c "! grep -q \"='@\" '$bin'"
check "deploy.env names the repository" grep -q "^NECROPSY_REPO=file://$work/origin.git" "$conf/deploy.env"
rule="$root/sudoers.d/necropsy-deploy"
check "sudo rule names the user" grep -q '^nobody ALL=(root) NOPASSWD: NECROPSY_DEPLOY' "$rule"
check "sudo rule allows deploy with a hex argument" grep -q "$bin deploy \[0-9a-f\]\*" "$rule"
check "sudo rule allows rollback and status" grep -q "$bin rollback, $bin status" "$rule"
check "sudo rule allows nothing else" bash -c "[ \$(grep -c '$bin' '$rule') -eq 1 ] && ! grep -q 'ALL$' '$rule'"
if command -v visudo >/dev/null; then
  check "visudo accepts the rule" visudo -cf "$rule"
else
  skipped "visudo not installed"
fi

check "installing as root is refused" bash -c "! '$installer' --user root --repo https://example.com/x --prefix $work/r1/usr --sysconfdir $work/r1/etc --statedir $work/r1/var --sudoers-dir $work/r1/s --unsafe-for-tests"
check "a missing user is refused" bash -c "! '$installer' --user no_such_user_xyz --repo https://example.com/x --prefix $work/r2/usr --sysconfdir $work/r2/etc --statedir $work/r2/var --sudoers-dir $work/r2/s --unsafe-for-tests"
check "an unsafe path is refused" bash -c "! '$installer' --user nobody --repo https://example.com/x --prefix '$work/r 3/usr' --sysconfdir $work/r3/etc --statedir $work/r3/var --sudoers-dir $work/r3/s --unsafe-for-tests"
check "--user is required" bash -c "! '$installer' --repo https://example.com/x --prefix $work/r4/usr --sysconfdir $work/r4/etc --statedir $work/r4/var --sudoers-dir $work/r4/s --unsafe-for-tests"

echo "keep" >> "$conf/necropsy.env"
echo "NECROPSY_PORT=8099" >> "$conf/deploy.env"
install_at t --unsafe-for-tests
check "reinstalling keeps the app settings" grep -q '^keep$' "$conf/necropsy.env"
check "reinstalling keeps deploy options" grep -q '^NECROPSY_PORT=8099$' "$conf/deploy.env"
sed -i '/^NECROPSY_PORT=8099$/d' "$conf/deploy.env"

echo "# arguments"
check "--help works without a daemon" env FAKE_NO_DAEMON=1 "$bin" --help
check "no arguments is an error" fails "$bin"
run frobnicate;                  check "an unknown command is refused" test $? -ne 0
run deploy;                      check "deploy needs a commit" test $? -ne 0
run deploy main;                 check "a branch name is not a commit" test $? -ne 0
run deploy "${sha_a:0:12}";      check "an abbreviated hash is refused" test $? -ne 0
run deploy "$(printf 'a%.0s' {1..40})"; check "an unknown commit is refused" test $? -ne 0
run status extra;                check "status takes no arguments" test $? -ne 0
run rollback now;                check "rollback takes no arguments" test $? -ne 0
FAKE_NO_DAEMON=1 run status;     check "no daemon is reported" test $? -ne 0
run switch 'evil/image:latest';  check "only necropsy images can be switched to" test $? -ne 0
run switch necropsy:missing;     check "an image that is not local is refused" test $? -ne 0
check "nothing was deployed by those" test ! -e "$state/active"

echo "# commits"
run deploy "$sha_side"
check "a commit that is not on main is refused" test $? -ne 0
check "and says why" out_has "not on main"
check "and nothing was built" test ! -d "$FAKE_DIR/builds"

run deploy "$sha_a"
check "deploying a commit on main succeeds" test $? -eq 0
tag_a="necropsy:${sha_a:0:12}"
check "the image is tagged by the commit" img_exists "$tag_a"
check "blue is live" equals "$(active)" blue
check "blue runs that image" equals "$(image_in necropsy-blue)" "$tag_a"
check "the build received the full commit hash" grep -q "^arg=GIT_SHA=$sha_a" "$FAKE_DIR/builds/$tag_a"
check "the build context has the repository files" grep -q './Dockerfile' "$FAKE_DIR/builds/$tag_a"
check "the build context has no git data" bash -c "! grep -q '\\.git' '$FAKE_DIR/builds/$tag_a'"
check "no build directory is left behind" bash -c "! ls '$state'/build.* >/dev/null 2>&1"
check "the app settings file was created private" test "$(stat -c %a "$conf/necropsy.env")" = 600
check "the proxy was started" running necropsy-proxy
check "proxy points at blue" grep -q 'to necropsy-blue:8080' "$state/proxy/upstream.caddy"
check "edge.caddy trusts the subnet gateway and loopback" grep -qx 'trusted_proxies static 172.30.30.1 127.0.0.1' "$state/proxy/edge.caddy"
check "history was recorded" grep -q 'live=blue' "$state/history.log"

# a new commit on main
echo two > "$src/file.txt"; git -C "$src" commit -q -am two; git -C "$src" push -q origin main
sha_b="$(git -C "$src" rev-parse HEAD)"; tag_b="necropsy:${sha_b:0:12}"
run deploy "$sha_b"
check "a new commit on main is picked up and deployed" test $? -eq 0
check "green is live" equals "$(active)" green
check "green runs the new image" equals "$(image_in necropsy-green)" "$tag_b"
check "the old color was stopped" bash -c "! grep -q true '$FAKE_DIR/c/necropsy-blue/running'"
check "the previous release is kept for rollback" img_exists "$tag_a"

FAKE_BUILD_FAIL=1 run deploy "$sha_a"
check "a failing build fails the deploy" test $? -ne 0
check "traffic stays on green" equals "$(active)" green
check "green is still running" running necropsy-green

echo "# switching, rollback, failures"
cp "$state/images.env" "$work/images.before"
run rollback
check "rollback succeeds" test $? -eq 0
check "blue is live after rollback" equals "$(active)" blue
check "blue runs the previous release" equals "$(image_in necropsy-blue)" "$tag_a"
check "green stopped after rollback" bash -c "! grep -q true '$FAKE_DIR/c/necropsy-green/running'"
run rollback
check "rolling back again returns to the newer release" equals "$(active)" green

release necropsy:t1
check "switch works in test mode" test $? -eq 0
check "blue is live (t1)" equals "$(active)" blue
cp "$state/images.env" "$work/images.before"

FAKE_UNHEALTHY=green release necropsy:t2
check "an unhealthy new color fails the deploy" test $? -ne 0
check "blue is still live" equals "$(active)" blue
check "proxy still points at blue" grep -q 'to necropsy-blue:8080' "$state/proxy/upstream.caddy"
check "blue is still running" running necropsy-blue
check "the failed color was stopped" bash -c "! grep -q true '$FAKE_DIR/c/necropsy-green/running'"
check "images.env is back to what it was" cmp -s "$state/images.env" "$work/images.before"
check "the failed color's log was shown" out_has 'fake log line'

FAKE_PROXY_BAD=1 release necropsy:t2
check "a rejected proxy config fails the deploy" test $? -ne 0
check "blue is still live (proxy validate)" equals "$(active)" blue
check "proxy still points at blue (proxy validate)" grep -q 'to necropsy-blue:8080' "$state/proxy/upstream.caddy"
check "images.env restored (proxy validate)" cmp -s "$state/images.env" "$work/images.before"

FAKE_LIVE_FAIL=1 release necropsy:t2
check "a site that does not answer fails the deploy" test $? -ne 0
check "blue is still live (live check)" equals "$(active)" blue
check "proxy points back at blue" grep -q 'to necropsy-blue:8080' "$state/proxy/upstream.caddy"
check "the proxy was reloaded back" grep -q 'necropsy-blue' "$FAKE_DIR/reloaded_upstream"
check "images.env restored (live check)" cmp -s "$state/images.env" "$work/images.before"

release necropsy:t3
check "a good deploy still works after those failures" test $? -eq 0
check "green is live" equals "$(active)" green
check "images no longer needed were pruned" bash -c "! test -e '$FAKE_DIR/images/$tag_b'"
check "--status names the live color" bash -c "'$bin' status | grep -q '^live:    green'"

echo "# settings file"
printf 'NECROPSY_PORT=9191\nFOO=$(touch %s/pwned)\nNECROPSY_EDGE_TRUSTED=10.9.9.9\n' "$work" >> "$conf/deploy.env"
release necropsy:t4
check "settings are read" bash -c "'$bin' status | grep -q 'port:    127.0.0.1:9191'"
check "a trusted-proxy setting reaches the proxy config" grep -qx 'trusted_proxies static 10.9.9.9' "$state/proxy/edge.caddy"
check "settings are never executed" test ! -e "$work/pwned"
printf 'NECROPSY_PORT=80a\n' >> "$conf/deploy.env"
run status;                      check "a bad port is refused" test $? -ne 0
sed -i '/^NECROPSY_PORT=80a$/d;/^NECROPSY_PORT=9191$/d;/^FOO=/d;/^NECROPSY_EDGE_TRUSTED=/d' "$conf/deploy.env"
printf "NECROPSY_EDGE_TRUSTED=1.2.3.4;rm\n" >> "$conf/deploy.env"
run status;                      check "an address list that is not addresses is refused" test $? -ne 0
sed -i '/^NECROPSY_EDGE_TRUSTED=/d' "$conf/deploy.env"

echo "# uninstall"
"$installer" --uninstall --prefix "$root/usr" --sysconfdir "$root/etc" --statedir "$root/var" --sudoers-dir "$root/sudoers.d" >/dev/null 2>&1
check "uninstall removes the program" test ! -e "$bin"
check "uninstall removes the sudo rule" test ! -e "$rule"
check "uninstall keeps the settings" test -f "$conf/necropsy.env"
check "uninstall keeps the state" test -f "$state/history.log"

# ---- the real mode: needs root ------------------------------------------------

echo "# safe mode (the real install)"
if [ "$(id -u)" -ne 0 ]; then
  skipped "needs root: run with sudo env PATH=\"\$PATH\" to include these"
else
  rm -rf "$FAKE_DIR/c" "$FAKE_DIR/images" "$FAKE_DIR/seen_docker_host"; mkdir -p "$FAKE_DIR/images"
  install_at s --repo https://example.invalid/necropsy
  sbin="$work/s/usr/bin/necropsy-deploy"
  sconf="$work/s/etc/necropsy"
  check "a real install is created" test -x "$sbin"
  check "the real installer refuses a non-https repository" bash -c "! '$installer' --user nobody --repo file:///x --prefix $work/s2/usr --sysconfdir $work/s2/etc --statedir $work/s2/var --sudoers-dir $work/s2/s --extra-path $fakebin"

  out="$("$sbin" switch necropsy:x 2>&1)"; check "switch does not exist in a real install" test $? -ne 0
  check "and says it is unknown" bash -c "echo '$out' | grep -q 'unknown command'"

  DOCKER_HOST=tcp://evil.invalid:1 GIT_DIR=/nonexistent BASH_ENV=/nonexistent LD_PRELOAD=/nonexistent "$sbin" status >/dev/null 2>&1
  check "the caller's DOCKER_HOST never reaches docker" equals "$(cat "$FAKE_DIR/seen_docker_host")" unset

  sed -i "s|^NECROPSY_REPO=.*|NECROPSY_REPO=file://$work/origin.git|" "$sconf/deploy.env"
  out="$("$sbin" deploy "$sha_a" 2>&1)"; check "a file:// repository is refused by the program in a real install" test $? -ne 0
  check "and says it must be https" bash -c "echo '$out' | grep -q 'https'"

  chmod 666 "$sconf/deploy.env"
  out="$("$sbin" status 2>&1)"; check "a world-writable settings file is refused" test $? -ne 0
  check "and says why" bash -c "echo '$out' | grep -q 'owned by root'"
  chmod 644 "$sconf/deploy.env"
  "$sbin" status >/dev/null 2>&1; check "it works again once fixed" test $? -eq 0

  "$sbin" --help >/dev/null 2>&1; check "--help works" test $? -eq 0
fi

echo
if [ "$failures" -eq 0 ]; then echo "all passed"; else echo "$failures failed"; exit 1; fi
