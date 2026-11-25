#!/usr/bin/env bash
# The lab's own test, against a running spawner: the site answers, the flag is
# where the puzzle hides it, the shell works only with its header, and the lab
# is the locked-down box the puzzle promises.
#
#   test.sh <spawner socket> <range dir> <image name in the allowlist>
#
# Run by deploy/range/ci-test.sh. Needs curl, and docker for the isolation checks.

set -euo pipefail

die() { echo "webshell-lab test: FAIL: $*" >&2; exit 1; }
ok() { echo "webshell-lab test: ok: $*" >&2; }

[ $# -eq 3 ] || { echo "usage: test.sh <spawner socket> <range dir> <image>" >&2; exit 2; }
socket="$1" range_dir="$2" image="$3"

id="wl$(od -An -N5 -tx1 /dev/urandom | tr -d ' \n')"
flag='shell{0123456789abcdef0123}'
api() { curl -fsS --max-time 60 --unix-socket "$socket" "$@"; }
trap 'api -X DELETE "http://spawner/v1/instances/$id" >/dev/null 2>&1 || true' EXIT

api -X POST http://spawner/v1/instances -H 'content-type: application/json' \
  -d "{\"id\":\"$id\",\"image\":\"$image\",\"ttl_secs\":180,\"env\":{\"NECROPSY_FLAG\":\"$flag\",\"NECROPSY_INSTANCE\":\"$id\"}}" >/dev/null \
  || die "the spawner could not start the lab"

lab=(curl -fsS --max-time 10 --unix-socket "$range_dir/$id/app.sock")
shell=(http://lab/wp-content/uploads/cache/.thumbs.php)

out="$("${lab[@]}" http://lab/)" || die "the site does not answer"
grep -q 'Greenleaf' <<<"$out" || die "the site is not the gardening blog"
ok "the site answers"

code="$(curl -sS --max-time 10 -o /dev/null -w '%{http_code}' --unix-socket "$range_dir/$id/app.sock" "${shell[@]}")"
[ "$code" = 404 ] || die "the shell answers without its header (HTTP $code)"
code="$(curl -sS --max-time 10 -o /dev/null -w '%{http_code}' --unix-socket "$range_dir/$id/app.sock" -H 'X-Auth: wrong' -d 'q=id' "${shell[@]}")"
[ "$code" = 404 ] || die "the shell answers with the wrong header (HTTP $code)"
ok "the shell is hidden without its header"

run() { "${lab[@]}" -H 'X-Auth: 4c1f9ab2' --data-urlencode "q=$1" "${shell[@]}"; }

out="$(run 'ls -a /tmp')"
grep -q '\.stage' <<<"$out" || die "the stash directory is not where the puzzle says (ls -a /tmp: $out)"
out="$(run 'cat /tmp/.stage/archive-0412.txt')"
grep -qF "$flag" <<<"$out" || die "the flag is not in the stash"
grep -q 'exfil_key' <<<"$out" || die "the stash does not look like one"
ok "the flag is in the stash"

[ "$(tr -d '[:space:]' <<<"$(run 'id -u')")" = 10001 ] || die "the shell is not running as the unprivileged user"
out="$(run 'touch /usr/x 2>&1; echo rc=$?')"
grep -q 'rc=[1-9]' <<<"$out" || die "the root filesystem is writable"
out="$(run 'wget -T 3 -q -O- http://1.1.1.1/ >/dev/null 2>&1 && echo ONLINE || echo OFFLINE')"
grep -q OFFLINE <<<"$out" || die "the lab can reach the internet"
ok "the shell runs unprivileged, on a read-only root, with no network"

# The puzzle's own brief says it can read the log and the sample: those are
# shipped files, so check they agree with the lab.
sample="$(dirname "$0")/../../packs/webshell-lab/files/thumbs.php.sample"
cmp -s "$sample" "$(dirname "$0")/site/wp-content/uploads/cache/.thumbs.php" \
  || die "the sample the player is given is not the shell the lab runs"
ok "the sample given to the player is the shell on the box"
