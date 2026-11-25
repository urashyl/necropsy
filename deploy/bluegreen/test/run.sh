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
img_exists() { [ -e "$FAKE_DIR/images/${1//\//_}" ]; }
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
check "deploy.env builds main by default" grep -qx "NECROPSY_BRANCH=main" "$conf/deploy.env"
check "a branch name that is not plain is refused" bash -c "! '$installer' --user nobody --repo https://example.com/x --branch 'a..b' --prefix $work/r5/usr --sysconfdir $work/r5/etc --statedir $work/r5/var --sudoers-dir $work/r5/s --unsafe-for-tests"
check "a branch name that looks like an option is refused" bash -c "! '$installer' --user nobody --repo https://example.com/x --branch '-x' --prefix $work/r6/usr --sysconfdir $work/r6/etc --statedir $work/r6/var --sudoers-dir $work/r6/s --unsafe-for-tests"
rule="$root/sudoers.d/necropsy-deploy"
check "sudo rule names the user and only the program" grep -qx "nobody ALL=(root) NOPASSWD: $bin" "$rule"
check "sudo rule grants exactly one command" bash -c "[ \$(grep -vc '^#' '$rule') -eq 1 ]"
check "sudo rule has no wildcards (sudo-rs rejects them)" bash -c "! grep -v '^#' '$rule' | grep -q '[*?\\[]'"
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
check "reinstalling keeps the branch" grep -qx "NECROPSY_BRANCH=main" "$conf/deploy.env"
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
proxy_ups() { grep -c '^up necropsy-proxy$' "$FAKE_DIR/calls"; }
ups="$(proxy_ups)"
release necropsy:t4
check "settings are read" bash -c "'$bin' status | grep -q 'port:    127.0.0.1:9191'"
check "a changed port recreates the proxy" equals "$(proxy_ups)" "$((ups + 1))"
check "the proxy is published on the new port" equals "$(cat "$FAKE_DIR/c/necropsy-proxy/published")" "127.0.0.1 9191"
check "a trusted-proxy setting reaches the proxy config" grep -qx 'trusted_proxies static 10.9.9.9' "$state/proxy/edge.caddy"
check "settings are never executed" test ! -e "$work/pwned"
printf 'NECROPSY_PORT=80a\n' >> "$conf/deploy.env"
run status;                      check "a bad port is refused" test $? -ne 0
sed -i '/^NECROPSY_PORT=80a$/d;/^NECROPSY_PORT=9191$/d;/^FOO=/d;/^NECROPSY_EDGE_TRUSTED=/d' "$conf/deploy.env"
printf "NECROPSY_EDGE_TRUSTED=1.2.3.4;rm\n" >> "$conf/deploy.env"
run status;                      check "an address list that is not addresses is refused" test $? -ne 0
sed -i '/^NECROPSY_EDGE_TRUSTED=/d' "$conf/deploy.env"

echo "# bind address"
release necropsy:t5
check "the proxy is published on loopback by default" equals "$(cat "$FAKE_DIR/c/necropsy-proxy/published")" "127.0.0.1 8080"
ups="$(proxy_ups)"
release necropsy:t6
check "an unchanged address reloads the proxy instead of recreating it" equals "$(proxy_ups)" "$ups"
echo "NECROPSY_BIND=0.0.0.0" >> "$conf/deploy.env"
release necropsy:t7
check "a deploy with a new bind address succeeds" test $? -eq 0
check "a new bind address recreates the proxy" equals "$(proxy_ups)" "$((ups + 1))"
check "the proxy is published on that address" equals "$(cat "$FAKE_DIR/c/necropsy-proxy/published")" "0.0.0.0 8080"
check "status shows the address" bash -c "'$bin' status | grep -q 'port:    0.0.0.0:8080'"
release necropsy:t8
check "and it is not recreated again" equals "$(proxy_ups)" "$((ups + 1))"
sed -i '/^NECROPSY_BIND=/d' "$conf/deploy.env"
for good in 10.0.0.2 :: ::1 fd00::1; do
  printf 'NECROPSY_BIND=%s\n' "$good" >> "$conf/deploy.env"
  run status; check "bind address $good is accepted" test $? -eq 0
  sed -i '/^NECROPSY_BIND=/d' "$conf/deploy.env"
done
for bad in localhost 0.0.0.0:80 1.2.3.256 1.2.3 '[::]' '::1%eth0' '1.2.3.4;rm' '1.2.3.4 5.6.7.8' ''; do
  printf 'NECROPSY_BIND=%s\n' "$bad" >> "$conf/deploy.env"
  run status; check "bind address '$bad' is refused" test $? -ne 0
  sed -i '/^NECROPSY_BIND=/d' "$conf/deploy.env"
done
for bad in 0 65536; do
  printf 'NECROPSY_PORT=%s\n' "$bad" >> "$conf/deploy.env"
  run status; check "port $bad is refused" test $? -ne 0
  sed -i '/^NECROPSY_PORT=/d' "$conf/deploy.env"
done
release necropsy:t9
check "back on loopback once the setting is removed" equals "$(cat "$FAKE_DIR/c/necropsy-proxy/published")" "127.0.0.1 8080"

echo "# a dev server (installed for another branch)"
rm -rf "$FAKE_DIR/c" "$FAKE_DIR/builds"
droot="$work/d"
install_at d --unsafe-for-tests --branch side
dbin="$droot/usr/bin/necropsy-deploy"
check "--branch is written to deploy.env" grep -qx "NECROPSY_BRANCH=side" "$droot/etc/necropsy/deploy.env"
printf 'DRAIN_SECONDS=0\nHEALTH_TIMEOUT=3\n' >> "$droot/etc/necropsy/deploy.env"
"$dbin" deploy "$sha_b" > "$work/out" 2>&1
check "a commit only on main is refused when installed for another branch" test $? -ne 0
check "and says which branch" out_has "not on side"
"$dbin" deploy "$sha_side" > "$work/out" 2>&1
check "a commit on the configured branch is built" grep -q "^arg=GIT_SHA=$sha_side" "$FAKE_DIR/builds/necropsy:${sha_side:0:12}"
install_at d --unsafe-for-tests
check "reinstalling without --branch keeps it" grep -qx "NECROPSY_BRANCH=side" "$droot/etc/necropsy/deploy.env"
sed -i 's/^NECROPSY_BRANCH=.*/NECROPSY_BRANCH=a..b/' "$droot/etc/necropsy/deploy.env"
"$dbin" deploy "$sha_side" > "$work/out" 2>&1
check "an invalid branch in deploy.env is refused" out_has "not a valid branch name"


echo "# the range (labs and spawner)"
# The commit gets a range: a Dockerfile naming the Rust version, an allowlist
# with one lab, the lab's Dockerfile, and a marker standing in for the spawner.
mkdir -p "$src/range/lab-a" "$src/range/lab-b"
printf 'ARG RUST_VERSION=1.100\nFROM scratch\n' > "$src/Dockerfile"
printf 'max_instances = 20\n\n[images.lab-a]\nimage = "necropsy-range/lab-a:1"\ncpus = 0.5\n\n[images.lab-b]\nimage = "necropsy-range/lab-b:1"\ncpus = 0.1\n' > "$src/range/images.toml"
echo 'FROM scratch' > "$src/range/lab-a/Dockerfile"; echo 'FROM scratch' > "$src/range/lab-b/Dockerfile"
echo one > "$src/spawner-marker.txt"
git -C "$src" add -A; git -C "$src" commit -q -m range; git -C "$src" push -q origin main
sha_r1="$(git -C "$src" rev-parse HEAD)"

rroot="$work/r"
rrun="$work/run"
mkdir -p "$rrun/necropsy"
install_at r --unsafe-for-tests --rundir "$rrun"
rbin="$rroot/usr/bin/necropsy-deploy"
rconf="$rroot/etc/necropsy"
rstate="$rroot/var/necropsy/bluegreen"
spawner_bin="$work/spawner-bin"
cat >> "$rconf/deploy.env" <<ENV
DRAIN_SECONDS=0
HEALTH_TIMEOUT=3
RANGE_UNIT=necropsy-spawner
RANGE_MAX_INSTANCES=7
RANGE_CPUS_CAP=0.25
RANGE_BUILD_SPAWNER=1
RANGE_SPAWNER_BIN=$spawner_bin
RANGE_RESTART_UNITS=necropsy-dev-spawner
ENV
# The test stands in for the self-test script: it records its arguments and
# fails when told to.
cat > "$rroot/usr/lib/necropsy/range-selftest.sh" <<'STUB'
#!/bin/sh
echo "$@" > "$FAKE_DIR/selftest_args"
[ ! -e "$FAKE_DIR/selftest_fail" ]
STUB
chmod +x "$rroot/usr/lib/necropsy/range-selftest.sh"
: > "$rrun/necropsy/spawner.sock"   # the spawner's socket, as far as the program can tell
rrun_() { "$rbin" "$@" > "$work/out" 2>&1; }
calls_since() { tail -n +"$(( $1 + 1 ))" "$FAKE_DIR/calls"; }
mark() { wc -l < "$FAKE_DIR/calls"; }

m="$(mark)"
rrun_ deploy "$sha_r1"
check "a deploy with a range succeeds" test $? -eq 0
check "the lab images are tagged by stack, lab and commit" img_exists "necropsy-range/necropsy/lab-a:${sha_r1:0:12}"
check "...both labs" img_exists "necropsy-range/necropsy/lab-b:${sha_r1:0:12}"
check "the allowlist points at them" grep -qx "image = \"necropsy-range/necropsy/lab-a:${sha_r1:0:12}\"" "$rconf/images.toml"
check "the stack's own limit replaces the repository's" grep -qx 'max_instances = 7' "$rconf/images.toml"
check "a higher cpu limit is lowered" grep -qx 'cpus = 0.25' "$rconf/images.toml"
check "a lower one is kept" grep -qx 'cpus = 0.1' "$rconf/images.toml"
check "the spawner was built" bash -c "calls_since() { tail -n +\"\$(( \$1 + 1 ))\" '$FAKE_DIR/calls'; }; calls_since $m | grep -q 'run spawner build'"
check "and installed" grep -q '^spawner$' "$spawner_bin"
check "both spawner units were restarted (the binary changed)" bash -c "calls_since() { tail -n +\"\$(( \$1 + 1 ))\" '$FAKE_DIR/calls'; }; calls_since $m | grep -q 'systemctl restart necropsy-spawner' && calls_since $m | grep -q 'systemctl restart necropsy-dev-spawner'"
check "the labs were tested with this stack's files" grep -q "$rconf/images.toml $rrun/necropsy/spawner.sock $rrun/necropsy/range" "$FAKE_DIR/selftest_args"
check "the site went live" equals "$(cat "$rstate/active")" blue
check "the program's own settings are not passed on to labs" bash -c "! grep -q RANGE_ '$rconf/images.toml'"

# Same commit again: nothing changed, nothing restarted.
m="$(mark)"
echo site-only-one > "$src/file.txt"; git -C "$src" commit -q -am "site only"; git -C "$src" push -q origin main
sha_r2="$(git -C "$src" rev-parse HEAD)"
rrun_ deploy "$sha_r2"
check "a site-only commit deploys" test $? -eq 0
check "...and changes the lab tag in the allowlist" grep -q "lab-a:${sha_r2:0:12}" "$rconf/images.toml"
check "the stack's spawner restarts on a new allowlist" bash -c "tail -n +\"\$(( $m + 1 ))\" '$FAKE_DIR/calls' | grep -q 'systemctl restart necropsy-spawner'"
check "but the other stack's spawner does not (the binary is the same)" bash -c "! tail -n +\"\$(( $m + 1 ))\" '$FAKE_DIR/calls' | grep -q 'restart necropsy-dev-spawner'"
check "the previous deploy's labs are kept for running labs" img_exists "necropsy-range/necropsy/lab-a:${sha_r1:0:12}"

# A third deploy prunes the first one's labs.
echo site-only-two > "$src/file.txt"; git -C "$src" commit -q -am "site only 2"; git -C "$src" push -q origin main
sha_r3="$(git -C "$src" rev-parse HEAD)"
rrun_ deploy "$sha_r3"
check "labs two deploys old are removed" bash -c "! test -e '$FAKE_DIR/images/necropsy-range_necropsy_lab-a:${sha_r1:0:12}'"
check "the last two deploys' labs remain" img_exists "necropsy-range/necropsy/lab-a:${sha_r2:0:12}"

# A spawner change.
m="$(mark)"
echo two > "$src/spawner-marker.txt"; git -C "$src" add -A; git -C "$src" commit -q -m "spawner"; git -C "$src" push -q origin main
sha_r4="$(git -C "$src" rev-parse HEAD)"
rrun_ deploy "$sha_r4"
check "a changed spawner is rebuilt and installed" grep -q '^two$' "$spawner_bin"
check "and restarts both units" bash -c "tail -n +\"\$(( $m + 1 ))\" '$FAKE_DIR/calls' | grep -q 'restart necropsy-dev-spawner'"

# Failures stop the deploy before the site changes.
live_before="$(cat "$rstate/active")"; img_before="$(cat "$rstate/images.env")"; imgs_before="$(cat "$rconf/images.toml")"
echo site-only-three > "$src/file.txt"; git -C "$src" commit -q -am "will fail"; git -C "$src" push -q origin main
sha_f="$(git -C "$src" rev-parse HEAD)"
: > "$FAKE_DIR/selftest_fail"
rrun_ deploy "$sha_f"
check "a lab that fails its test fails the deploy" test $? -ne 0
check "and says the site was not changed" out_has "site was not changed"
check "the previous allowlist is put back" equals "$(cat "$rconf/images.toml")" "$imgs_before"
check "and the spawner is restarted on it" bash -c "tail -n 2 '$FAKE_DIR/calls' | grep -q 'restart necropsy-spawner'"
check "no leftovers from the failed attempt" bash -c "! ls '$rconf'/images.toml.* >/dev/null 2>&1"
check "the live color is unchanged" equals "$(cat "$rstate/active")" "$live_before"
check "and so are the images" equals "$(cat "$rstate/images.env")" "$img_before"
rm -f "$FAKE_DIR/selftest_fail"
FAKE_SPAWNER_BUILD_FAIL=1 rrun_ deploy "$sha_f"
check "a spawner that does not build fails the deploy" test $? -ne 0
FAKE_SYSTEMCTL_FAIL=1 rrun_ deploy "$sha_f"
check "a unit that will not restart fails the deploy" test $? -ne 0
check "...without changing the site" equals "$(cat "$rstate/active")" "$live_before"
rm -f "$rrun/necropsy/spawner.sock"
rrun_ deploy "$sha_f"
check "a spawner without a socket fails the deploy" test $? -ne 0
: > "$rrun/necropsy/spawner.sock"
rrun_ deploy "$sha_f"
check "and the next deploy works once it is fixed" test $? -eq 0

# An allowlist naming a lab that has no image.
git -C "$src" show "$sha_f:range/images.toml" > "$work/good-images.toml"
printf 'max_instances = 20\n\n[images.ghost]\nimage = "necropsy-range/ghost:1"\n' > "$src/range/images.toml"
git -C "$src" commit -q -am "ghost"; git -C "$src" push -q origin main
rrun_ deploy "$(git -C "$src" rev-parse HEAD)"
check "an allowlist naming a lab with no Dockerfile is refused" test $? -ne 0
printf 'max_instances = 20\n\n[images.evil]\nimage = "alpine:latest"\n' > "$src/range/images.toml"
git -C "$src" commit -q -am "evil"; git -C "$src" push -q origin main
rrun_ deploy "$(git -C "$src" rev-parse HEAD)"
check "an allowlist naming an outside image is refused" test $? -ne 0
check "and says so" out_has "not one of this commit"
cp "$work/good-images.toml" "$src/range/images.toml"; git -C "$src" commit -q -am "restore the allowlist"; git -C "$src" push -q origin main

# Settings are checked.
for bad in 'RANGE_UNIT=a b' 'RANGE_UNIT=../x' 'RANGE_RESTART_UNITS=ok;rm' 'RANGE_MAX_INSTANCES=0' 'RANGE_MAX_INSTANCES=lots' 'RANGE_CPUS_CAP=fast' 'RANGE_BUILD_SPAWNER=2' 'RANGE_SPAWNER_BIN=bin/x'; do
  cp "$rconf/deploy.env" "$work/deploy.env.bak"
  printf '%s\n' "$bad" >> "$rconf/deploy.env"
  rrun_ status; check "'$bad' is refused" test $? -ne 0
  cp "$work/deploy.env.bak" "$rconf/deploy.env"
done
rrun_ status
check "status shows the range" out_has 'range:'

echo "# two stacks on one host"
# Production above and a dev stack here share the fake docker: names, images,
# volumes and ports must not collide, and one stack's cleanup must not touch the other's.
d2root="$work/d2"
install_at d2 --unsafe-for-tests --name necropsy-dev --branch main --rundir "$rrun"
d2bin="$d2root/usr/bin/necropsy-dev-deploy"
d2conf="$d2root/etc/necropsy-dev"
d2state="$d2root/var/necropsy-dev/bluegreen"
check "the dev program has its own name" test -x "$d2bin"
check "...and its own sudo rule" test -f "$d2root/sudoers.d/necropsy-dev-deploy"
check "the production program is still there" test -x "$rbin"
check "the sudo rule names the dev program" grep -qx "nobody ALL=(root) NOPASSWD: $d2bin" "$d2root/sudoers.d/necropsy-dev-deploy"
check "the dev settings are in their own directory" test -f "$d2conf/necropsy.env"
check "the dev program has the stack name baked in" grep -q "^NAME='necropsy-dev'" "$d2bin"
check "a bad name is refused" bash -c "! '$installer' --user nobody --repo https://example.com/x --name 'Bad Name' --prefix $work/r7/usr --sysconfdir $work/r7/etc --statedir $work/r7/var --sudoers-dir $work/r7/s --unsafe-for-tests"
cat >> "$d2conf/deploy.env" <<ENV
DRAIN_SECONDS=0
HEALTH_TIMEOUT=3
NECROPSY_PORT=8081
NECROPSY_SUBNET=172.30.31.0/24
RANGE_UNIT=necropsy-dev-spawner
RANGE_MAX_INSTANCES=3
ENV
mkdir -p "$rrun/necropsy-dev"; : > "$rrun/necropsy-dev/spawner.sock"
cp "$rroot/usr/lib/necropsy/range-selftest.sh" "$d2root/usr/lib/necropsy-dev/range-selftest.sh"
prod_live="$(cat "$rstate/active")"; prod_img="$(cat "$rstate/images.env")"
m="$(mark)"
sha_now="$(git -C "$src" rev-parse HEAD)"
"$d2bin" deploy "$sha_now" > "$work/out" 2>&1
check "the dev stack deploys beside production" test $? -eq 0
check "its containers are named for it" test -d "$FAKE_DIR/c/necropsy-dev-blue"
check "...and its proxy" test -d "$FAKE_DIR/c/necropsy-dev-proxy"
check "its image repository is its own" img_exists "necropsy-dev:${sha_now:0:12}"
check "its labs are its own" img_exists "necropsy-range/necropsy-dev/lab-a:${sha_now:0:12}"
check "its allowlist has its own limit" grep -qx 'max_instances = 3' "$d2conf/images.toml"
check "production's allowlist is untouched" grep -qx 'max_instances = 7' "$rconf/images.toml"
check "the dev proxy is published on its own port" equals "$(cat "$FAKE_DIR/c/necropsy-dev-proxy/published")" "127.0.0.1 8081"
check "the dev proxy points at the dev container" grep -q 'to necropsy-dev-blue:8080' "$d2state/proxy/upstream.caddy"
check "production's state did not move" equals "$(cat "$rstate/active")" "$prod_live"
check "production's images did not change" equals "$(cat "$rstate/images.env")" "$prod_img"
check "production's proxy still points at production" grep -q "to necropsy-" "$rstate/proxy/upstream.caddy"
check "...not at dev" bash -c "! grep -q 'necropsy-dev' '$rstate/proxy/upstream.caddy'"
check "the dev deploy restarted only its own spawner" bash -c "tail -n +\"\$(( $m + 1 ))\" '$FAKE_DIR/calls' | grep -q 'restart necropsy-dev-spawner' && ! tail -n +\"\$(( $m + 1 ))\" '$FAKE_DIR/calls' | grep -q 'restart necropsy-spawner'"
check "the dev stack does not rebuild the spawner program" bash -c "! tail -n +\"\$(( $m + 1 ))\" '$FAKE_DIR/calls' | grep -q 'run spawner build'"
check "dev refuses an image of production's" bash -c "! '$d2bin' switch necropsy:x"
check "the dev program tells its own name" bash -c "'$d2bin' deploy 2>&1 | grep -q 'necropsy-dev-deploy deploy'"
# Two more dev deploys, then production's images must still all be there.
for i in 1 2; do echo "d$i" > "$src/file.txt"; git -C "$src" commit -q -am "d$i"; git -C "$src" push -q origin main; "$d2bin" deploy "$(git -C "$src" rev-parse HEAD)" >/dev/null 2>&1; done
check "pruning dev's images leaves production's alone" bash -c "[ \"\$(cat '$rstate/images.env')\" = \"$prod_img\" ] && for ref in \$(sed 's/^[A-Z]*_IMAGE=//' '$rstate/images.env'); do test -e '$FAKE_DIR/images/'\"\$ref\"; done"
check "pruning dev's labs leaves production's alone" img_exists "necropsy-range/necropsy/lab-a:${sha_r4:0:12}"

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
