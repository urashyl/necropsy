#!/usr/bin/env bash
# The lab's own test, against a running spawner: the console answers only with its
# token; ps, top and ls /proc really do hide the process; reading /proc directly
# finds it; its environment holds the flag, and nothing visible does. Everything goes
# through the spawner's lab proxy, the way a site's traffic does.
#
#   test.sh <spawner url> <tenant token> <lab name in the allowlist>
#
# Run by deploy/range/ci-test.sh. Needs curl.

set -euo pipefail

die() { echo "hidden-process-lab test: FAIL: $*" >&2; exit 1; }
ok() { echo "hidden-process-lab test: ok: $*" >&2; }

[ $# -eq 3 ] || { echo "usage: test.sh <spawner url> <token> <lab>" >&2; exit 2; }
base="$1" token="$2" lab="$3"

id="hp$(od -An -N5 -tx1 /dev/urandom | tr -d ' \n')"
flag='hidden{0123456789abcdef0123}'
api() { curl -fsS --max-time 60 -H "X-Necropsy-Token: $token" "$@"; }
trap 'api -X DELETE "$base/v1/instances/$id" >/dev/null 2>&1 || true' EXIT

api -X POST "$base/v1/instances" -H 'content-type: application/json' \
  -d "{\"id\":\"$id\",\"image\":\"$lab\",\"ttl_secs\":180,\"env\":{\"NECROPSY_FLAG\":\"$flag\",\"NECROPSY_INSTANCE\":\"$id\"}}" >/dev/null \
  || die "the spawner could not start the lab"

lab_curl() { curl -sS --max-time 15 -H "X-Necropsy-Token: $token" -H "X-Necropsy-Instance: $id" "$@"; }
console="$base/console.php"

out=""
for _ in $(seq 1 50); do
  if out="$(lab_curl -f "$base/" 2>/dev/null)"; then break; fi
  sleep 0.2
done
grep -q 'Ironbridge' <<<"$out" || die "the site does not answer, or is not the gym's"
ok "the site answers"

code="$(lab_curl -o /dev/null -w '%{http_code}' -d 'q=id' "$console")"
[ "$code" = 401 ] || die "the console answers without its token (HTTP $code)"
run() { lab_curl -f -H 'X-Console: support-7f3a' --data-urlencode "q=$1" "$console"; }

# A command that never returns (someone's `grep -R x /`) must not freeze the lab for
# the next request.
( run 'sleep 25' >/dev/null 2>&1 & ) 
sleep 1
[ "$(run 'echo alive' | tr -d '[:space:]')" = alive ] || die "a hung command blocked the lab"
ok "a hung command does not block the lab"
[ "$(tr -d '[:space:]' <<<"$(run 'id -u')")" = 10001 ] || die "the console is not running as the unprivileged user"
ok "the console needs its token, and runs unprivileged"

# The hider is in place, and it really hides.
grep -qx '/usr/lib/libc.so.7' <<<"$(run 'cat /etc/ld.so.preload')" || die "the preload file is not what the puzzle says"
out="$(run 'ps -eo pid,args')"
grep -q 'php -S' <<<"$out" || die "ps does not show the web server (ps: $out)"
grep -q sysupd <<<"$out" && die "ps shows the hidden process"
grep -q sysupd <<<"$(run 'ps aux')" && die "ps aux shows the hidden process"
ok "ps does not show it"

# Guessing pids finds it, the way the puzzle expects.
pid="$(tr -d '[:space:]' <<<"$(run 'for i in $(seq 1 500); do tr "\\0" " " < /proc/$i/cmdline 2>/dev/null | grep -q sysupd && echo $i; done | head -n 1')")"
[[ "$pid" =~ ^[0-9]+$ ]] || die "reading /proc/<pid>/cmdline directly does not find it"
grep -qx "$pid" <<<"$(run 'ls /proc')" && die "ls /proc shows the hidden pid $pid"
ok "ls /proc does not show it, but pid $pid is there when asked for directly"

grep -qF "EXFIL_KEY=$flag" <<<"$(run "tr '\\0' '\\n' < /proc/$pid/environ")" || die "the hidden process's environment does not hold the key"
ok "its environment holds the key"

# And nothing a player can see holds it, PID 1 included.
leak="$(run 'for p in $(ls /proc | grep -E "^[0-9]+$") 1; do tr "\\0" "\\n" < /proc/$p/environ 2>/dev/null; done')"
grep -qF "$flag" <<<"$leak" && die "the key is in the environment of a visible process"
grep -q 'NECROPSY_FLAG' <<<"$leak" && die "NECROPSY_FLAG is in a visible process's environment"
ok "no visible process, PID 1 included, has the key"

out="$(run 'touch /usr/x 2>&1; echo rc=$?')"
grep -q 'rc=[1-9]' <<<"$out" || die "the root filesystem is writable"
out="$(run 'timeout 3 bash -c "exec 3<>/dev/tcp/1.1.1.1/80" >/dev/null 2>&1 && echo ONLINE || echo OFFLINE')"
grep -q OFFLINE <<<"$out" || die "the lab can reach the internet"
ok "read-only root, no network"
