#!/usr/bin/env bash
# Tests deploy/lib/common.sh, the helpers both installers share.
#
#   deploy/test/common.sh

set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
lib="$here/../lib/common.sh"
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

# Run a snippet with the library loaded, in its own shell (it is written for scripts that set -e).
fn() { bash -c 'set -euo pipefail; . "$0"; '"$1" "$lib" "${@:2}"; }

# ---- validators -------------------------------------------------------------------

for good in necropsy.urashyl.dev a.b.co x-y.example.org; do check "domain $good is valid" fn 'valid_domain "$1"' "$good"; done
for bad in localhost 'a b.org' 'A.org' 'a.org/x' '.a.org' 'a..org' '-a.org' 'a.o'; do check "domain '$bad' is refused" fails fn 'valid_domain "$1"' "$bad"; done
for good in 10.0.0.1 192.168.100.50 255.255.255.255 0.0.0.0; do check "ipv4 $good is valid" fn 'valid_ipv4 "$1"' "$good"; done
for bad in 256.1.1.1 1.2.3 1.2.3.4.5 a.b.c.d 1.2.3.999 '' '::1' '10.0.0.1/24'; do check "ipv4 '$bad' is refused" fails fn 'valid_ipv4 "$1"' "$bad"; done
for good in 10.0.0.1 ::1 fd00::1; do check "address $good is valid" fn 'valid_addr "$1"' "$good"; done
for bad in nope 10.0.0.1,10.0.0.2 '[::1]' ''; do check "address '$bad' is refused" fails fn 'valid_addr "$1"' "$bad"; done
for good in main dev feature/x release-1.2; do check "branch $good is valid" fn 'valid_branch "$1"' "$good"; done
for bad in '-x' 'a..b' '' 'a b' '../x'; do check "branch '$bad' is refused" fails fn 'valid_branch "$1"' "$bad"; done
check "root is not a valid deploy user" fails fn 'valid_user root'
check "deploy is" fn 'valid_user deploy'
check "Deploy is not" fails fn 'valid_user Deploy'
for good in prod dev a1 abcdefghij012345; do check "tenant $good is valid" fn 'valid_tenant "$1"' "$good"; done
for bad in Prod 1a pr-od '' abcdefghijklmnopq 'a b'; do check "tenant '$bad' is refused" fails fn 'valid_tenant "$1"' "$bad"; done

# ---- the settings editor --------------------------------------------------------

env_file="$work/env"
cat > "$env_file" <<'ENV'
# comment
#NECROPSY_LIVE_DOMAIN=necropsy.urashyl.dev
#NECROPSY_LIVE_DOMAIN=necropsy.localhost:8080
NECROPSY_BASE_URL=https://old.example
#NECROPSY_SMTP_FROM="necropsy <no-reply@example.com>"
ENV
chmod 600 "$env_file"
fn 'set_env "$1" NECROPSY_LIVE_DOMAIN example.org' "$env_file"
fn 'set_env "$1" NECROPSY_BASE_URL https://example.org' "$env_file"
fn 'set_env "$1" NECROPSY_NEW value' "$env_file"
fn 'set_env "$1" NECROPSY_SMTP_FROM "necropsy <a@b.c>"' "$env_file"
check "a commented example is turned on"         grep -qx 'NECROPSY_LIVE_DOMAIN=example.org' "$env_file"
check "only the first commented example is used" equals "$(grep -c 'NECROPSY_LIVE_DOMAIN' "$env_file")" 2
check "the other example is kept"                grep -qx '#NECROPSY_LIVE_DOMAIN=necropsy.localhost:8080' "$env_file"
check "an existing line is replaced"             grep -qx 'NECROPSY_BASE_URL=https://example.org' "$env_file"
check "...not duplicated"                        equals "$(grep -c '^NECROPSY_BASE_URL' "$env_file")" 1
check "a missing key is appended"                grep -qx 'NECROPSY_NEW=value' "$env_file"
check "a value with spaces is quoted"            grep -qx 'NECROPSY_SMTP_FROM="necropsy <a@b.c>"' "$env_file"
check "the mode is kept"                         equals "$(stat -c %a "$env_file")" 600
fn 'set_env "$1" NECROPSY_X "a"; set_env "$1" NECROPSY_X "b"' "$env_file"
check "setting twice leaves one line"            equals "$(grep -c '^NECROPSY_X=' "$env_file")" 1
for bad in 'a$b' "a'b" 'a`b' 'a\b' $'a\nb'; do
  check "a value like ${bad//$'\n'/NEWLINE} is refused" fails fn 'set_env "$1" NECROPSY_Y "$2"' "$env_file" "$bad"
done
check "nothing refused was written"              fails grep -q NECROPSY_Y "$env_file"

# ---- run and write_file honour a dry run ----------------------------------------------

out="$(fn 'dry=1; run touch "$1/should-not-exist"; echo hi | write_file "$1/nor-this" 644' "$work" 2>&1)"
check "a dry run runs nothing"                   test ! -e "$work/should-not-exist"
check "...and writes nothing"                    test ! -e "$work/nor-this"
check "...but says what it would do"             grep -q '+ touch' <<<"$out"
fn 'dry=0; echo hello | write_file "$1/real" 640' "$work"
check "a real write makes the file"              equals "$(cat "$work/real")" hello
check "...with the mode"                         equals "$(stat -c %a "$work/real")" 640
check "...and leaves no temp file behind"        equals "$(ls "$work" | grep -c 'real\.')" 0

echo
if [ "$failures" -eq 0 ]; then echo "all passed"; else echo "$failures failed"; exit 1; fi
