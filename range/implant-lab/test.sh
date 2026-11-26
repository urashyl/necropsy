#!/usr/bin/env bash
# The lab's own test, against a running spawner: the workbench answers, no page holds
# the flag, and the full protocol (solve.py) works through the spawner's lab proxy and
# ends with this instance's flag.
#
#   test.sh <spawner url> <tenant token> <lab name in the allowlist>
#
# Run by deploy/range/ci-test.sh. Needs curl and python3.

set -euo pipefail

die() { echo "implant-lab test: FAIL: $*" >&2; exit 1; }
ok() { echo "implant-lab test: ok: $*" >&2; }

[ $# -eq 3 ] || { echo "usage: test.sh <spawner url> <token> <lab>" >&2; exit 2; }
base="$1" token="$2" lab="$3"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

id="il$(od -An -N5 -tx1 /dev/urandom | tr -d ' \n')"
flag='implant{0123456789abcdef0123}'
api() { curl -fsS --max-time 60 -H "X-Necropsy-Token: $token" "$@"; }
trap 'api -X DELETE "$base/v1/instances/$id" >/dev/null 2>&1 || true' EXIT

api -X POST "$base/v1/instances" -H 'content-type: application/json' \
  -d "{\"id\":\"$id\",\"image\":\"$lab\",\"ttl_secs\":180,\"env\":{\"NECROPSY_FLAG\":\"$flag\",\"NECROPSY_INSTANCE\":\"$id\"}}" >/dev/null \
  || die "the spawner could not start the lab"

lab_curl() { curl -sS --max-time 15 -H "X-Necropsy-Token: $token" -H "X-Necropsy-Instance: $id" "$@"; }

out=""
for _ in $(seq 1 50); do
  if out="$(lab_curl -f "$base/" 2>/dev/null)"; then break; fi
  sleep 0.2
done
grep -q 'Sample triage' <<<"$out" || die "the workbench does not answer"
ok "the workbench answers"

for p in / /samples/helper-update.py.txt /captures/ws0412.cap; do
  grep -qF "$flag" <<<"$(lab_curl "$base$p")" && die "$p contains the flag"
done
ok "no page holds the flag"

got="$(python3 "$here/solve.py" "$base" --header "X-Necropsy-Token: $token" --header "X-Necropsy-Instance: $id" --flag "$flag")" \
  || die "the protocol walkthrough failed"
[ "$got" = "$flag" ] || die "the walkthrough ended with $got"
ok "the protocol works end to end and ends with the instance's flag"
