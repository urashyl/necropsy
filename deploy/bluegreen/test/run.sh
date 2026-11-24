#!/usr/bin/env bash
# Tests deploy.sh's switching logic against a fake docker (see fakebin/docker).
# Needs no Docker daemon, so it runs anywhere:  deploy/bluegreen/test/run.sh

set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$here/../deploy.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

export FAKE_DIR="$work/fake"
export HOME="$work/home"
export PATH="$here/fakebin:$PATH"
export DRAIN_SECONDS=0
export HEALTH_TIMEOUT=3
unset XDG_CONFIG_HOME XDG_STATE_HOME XDG_DATA_HOME NECROPSY_PORT NECROPSY_SUBNET NECROPSY_EDGE_TRUSTED
mkdir -p "$HOME"

state="$HOME/.local/state/necropsy/bluegreen"
failures=0

check() { # description, command...
  local what="$1"; shift
  if "$@" >/dev/null 2>&1; then echo "ok   $what"; else echo "FAIL $what"; failures=$((failures + 1)); fi
}
equals() { [ "$1" = "$2" ]; }
add_image() { : > "$FAKE_DIR/images/$1"; }
deploy() { "$script" "$@" > "$work/out" 2>&1; }
# Deploys a release whose image was just loaded, as CI does.
release() { add_image "$1"; deploy "$1"; }
active() { cat "$state/active" 2>/dev/null; }
upstream() { cat "$state/proxy/upstream.conf" 2>/dev/null; }
running() { [ "$(cat "$FAKE_DIR/c/$1/running" 2>/dev/null)" = true ]; }
image_in() { cat "$FAKE_DIR/c/$1/image" 2>/dev/null; }
img_exists() { [ -e "$FAKE_DIR/images/$1" ]; }

mkdir -p "$FAKE_DIR/images"

echo "# help works without a daemon"
check "--help exits 0 even with no daemon" env FAKE_NO_DAEMON=1 "$script" --help
check "no arguments is an error" bash -c "! '$script'"

echo "# argument and environment checks"
deploy necropsy:missing;                         check "unknown image is refused" test $? -ne 0
NECROPSY_PORT=80a release necropsy:t1;            check "bad port is refused" test $? -ne 0
NECROPSY_EDGE_TRUSTED='1.2.3.4;rm' release necropsy:t1; check "edge entry that is not an address is refused" test $? -ne 0
FAKE_NO_DAEMON=1 release necropsy:t1;             check "no daemon is reported" test $? -ne 0
check "nothing was deployed by those" test ! -e "$state/active"

echo "# first deploy"
release necropsy:t1
check "first deploy succeeds" test $? -eq 0
check "blue is live" equals "$(active)" blue
check "blue runs t1" equals "$(image_in necropsy-blue)" necropsy:t1
check "the proxy was started" running necropsy-proxy
check "proxy points at blue" grep -q 'necropsy-blue:8080' "$state/proxy/upstream.conf"
check "edge.conf trusts the subnet gateway" grep -q '^set_real_ip_from 172.30.30.1;' "$state/proxy/edge.conf"
check "edge.conf trusts loopback" grep -q '^set_real_ip_from 127.0.0.1;' "$state/proxy/edge.conf"
check "settings file was created private" test "$(stat -c %a "$HOME/.config/necropsy/necropsy.env")" = 600
check "history was recorded" grep -q 'live=blue' "$state/history.log"

echo "# second deploy switches color"
release necropsy:t2
check "second deploy succeeds" test $? -eq 0
check "green is live" equals "$(active)" green
check "green runs t2" equals "$(image_in necropsy-green)" necropsy:t2
check "proxy points at green" grep -q 'necropsy-green:8080' "$state/proxy/upstream.conf"
check "the old color was stopped" bash -c "! grep -q true '$FAKE_DIR/c/necropsy-blue/running'"
check "proxy was reloaded onto green" grep -q 'necropsy-green' "$FAKE_DIR/reloaded_upstream"
check "the previous release is kept for rollback" img_exists necropsy:t1

echo "# third deploy goes back to blue and prunes the oldest image"
release necropsy:t3
check "third deploy succeeds" test $? -eq 0
check "blue is live again" equals "$(active)" blue
check "blue runs t3" equals "$(image_in necropsy-blue)" necropsy:t3
check "t1 was pruned" bash -c "! test -e '$FAKE_DIR/images/necropsy:t1'"
check "t2 is kept as the rollback target" img_exists necropsy:t2

echo "# rollback"
deploy --rollback
check "rollback succeeds" test $? -eq 0
check "green is live after rollback" equals "$(active)" green
check "green runs t2" equals "$(image_in necropsy-green)" necropsy:t2
check "blue stopped after rollback" bash -c "! grep -q true '$FAKE_DIR/c/necropsy-blue/running'"

echo "# a new color that never becomes healthy changes nothing"
cp "$state/images.env" "$work/images.before"
FAKE_UNHEALTHY=blue release necropsy:t4
check "unhealthy deploy fails" test $? -ne 0
check "green is still live" equals "$(active)" green
check "proxy still points at green" grep -q 'necropsy-green:8080' "$state/proxy/upstream.conf"
check "green is still running" running necropsy-green
check "the failed color was stopped" bash -c "! grep -q true '$FAKE_DIR/c/necropsy-blue/running'"
check "images.env is back to what it was" cmp -s "$state/images.env" "$work/images.before"
check "the log of the failed color was shown" grep -q 'fake log line' "$work/out"

echo "# a proxy that rejects the config changes nothing"
FAKE_NGINX_BAD=1 release necropsy:t4
check "rejected config fails the deploy" test $? -ne 0
check "green is still live (nginx -t)" equals "$(active)" green
check "proxy still points at green (nginx -t)" grep -q 'necropsy-green:8080' "$state/proxy/upstream.conf"
check "images.env restored (nginx -t)" cmp -s "$state/images.env" "$work/images.before"

echo "# a site that does not answer after the switch is switched back"
FAKE_LIVE_FAIL=1 release necropsy:t4
check "failed live check fails the deploy" test $? -ne 0
check "green is still live (live check)" equals "$(active)" green
check "proxy points back at green" grep -q 'necropsy-green:8080' "$state/proxy/upstream.conf"
check "green was reloaded back" grep -q 'necropsy-green' "$FAKE_DIR/reloaded_upstream"
check "images.env restored (live check)" cmp -s "$state/images.env" "$work/images.before"

echo "# a good deploy still works after those failures"
release necropsy:t5
check "deploy after failures succeeds" test $? -eq 0
check "blue is live" equals "$(active)" blue
check "blue runs t5" equals "$(image_in necropsy-blue)" necropsy:t5

echo "# status"
check "--status runs and names the live color" bash -c "'$script' --status | grep -q '^live:    blue'"

echo
if [ "$failures" -eq 0 ]; then echo "all passed"; else echo "$failures failed"; exit 1; fi
