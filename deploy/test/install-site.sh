#!/usr/bin/env bash
# Tests deploy/install-site.sh: its argument checks, its real-mode settings, its
# check of the range VM, and a full --dry-run. Nothing here installs anything.
#
#   deploy/test/install-site.sh

set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$here/../install-site.sh"
repo_root="$(cd "$here/../.." && pwd)"
work="$(mktemp -d)"
fake_pid=""
trap '[ -z "$fake_pid" ] || kill "$fake_pid" 2>/dev/null; rm -rf "$work"' EXIT
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

# The script's functions, in their own shell (it does not run main when sourced).
fn() { NECROPSY_INSTALL_SOURCED=1 bash -c '. "$0"; '"$1" "$script" "${@:2}"; }
# The script itself, as a dry run.
dry() { bash "$script" --dry-run "$@"; }

TOKEN=0123456789abcdef0123456789abcdef
printf '%s\n' "$TOKEN" > "$work/token"
echo secret > "$work/secret"
ok_args=(--domain a.example.org --proxy-ip 10.0.0.5 --repo https://github.com/o/r)
range_args=(--range-url http://10.9.9.9:9400 --range-tenant prod --range-token-file "$work/token")

# ---- argument checks ------------------------------------------------------------

check "no arguments is refused"                 fails dry
check "a domain alone is refused (no proxy)"    fails dry --domain necropsy.example.org
check "a bad domain is refused"                 fails dry --domain 'Not A Domain' --proxy-ip 10.0.0.5
check "a domain with a path is refused"         fails dry --domain 'a.example.org/x' --proxy-ip 10.0.0.5
check "a bad proxy address is refused"          fails dry --domain a.example.org --proxy-ip 'nope'
check "a list of proxies is refused"            fails dry --domain a.example.org --proxy-ip 10.0.0.5,10.0.0.6
check "both proxy options are refused"          fails dry --domain a.example.org --proxy-ip 10.0.0.5 --proxy-on-host
check "root as the deploy user is refused"      fails dry "${ok_args[@]}" --deploy-user root
check "a bad branch is refused"                 fails dry "${ok_args[@]}" --branch '../x'
check "an http repo is refused"                 fails dry --domain a.example.org --proxy-ip 10.0.0.5 --repo http://github.com/o/r
check "a site key without a secret is refused"  fails dry "${ok_args[@]}" --turnstile-site-key 0xAB
check "a secret without a site key is refused"  fails dry "${ok_args[@]}" --turnstile-secret-file "$work/secret"
check "an unknown option is refused"            fails dry "${ok_args[@]}" --frobnicate
check "--help works"                            bash "$script" --help
check "the range options go together (url alone)"   fails dry "${ok_args[@]}" --range-url http://10.9.9.9:9400
check "...(tenant alone)"                       fails dry "${ok_args[@]}" --range-tenant prod
check "...(token alone)"                        fails dry "${ok_args[@]}" --range-token-file "$work/token"
check "all three together are fine"             dry "${ok_args[@]}" "${range_args[@]}"
check "a range url that is not http is refused" fails dry "${ok_args[@]}" --range-url ftp://x --range-tenant prod --range-token-file "$work/token"
check "a range url with a path is refused"      fails dry "${ok_args[@]}" --range-url http://10.9.9.9:9400/x --range-tenant prod --range-token-file "$work/token"
check "a bad tenant is refused"                 fails dry "${ok_args[@]}" --range-url http://10.9.9.9:9400 --range-tenant Prod --range-token-file "$work/token"
check "a token file that is missing is refused" fails dry "${ok_args[@]}" --range-url http://10.9.9.9:9400 --range-tenant prod --range-token-file "$work/nope"
echo short > "$work/short"
check "a token that is too short is refused"    fails dry "${ok_args[@]}" --range-url http://10.9.9.9:9400 --range-tenant prod --range-token-file "$work/short"
printf 'has spaces in the middle of it 0123456789' > "$work/odd"
check "a token with odd characters is refused"  fails dry "${ok_args[@]}" --range-url http://10.9.9.9:9400 --range-tenant prod --range-token-file "$work/odd"

# ---- the real-mode steps, against a temporary /etc ---------------------------------
#
# Everything that would change the machine goes through `run`, which is stubbed
# out here; the files the script writes itself go to a temporary directory.

conf="$work/etc"
mkdir -p "$conf"
cp "$repo_root/.env.example" "$conf/necropsy.env"
chmod 600 "$conf/necropsy.env"
printf '#NECROPSY_BIND=127.0.0.1\n#NECROPSY_EDGE_TRUSTED=172.30.30.1,127.0.0.1\nNECROPSY_REPO=https://github.com/o/r\n' > "$conf/deploy.env"
echo '0xTHESECRET' > "$work/ts"

settings() { # extra shell run first
  NECROPSY_INSTALL_SOURCED=1 NECROPSY_SMTP_PASSWORD='p4ss' bash -c '
    . "$0"; CONF_DIR="$1"; dry=0; run() { :; }
    domain=necropsy.example.org; proxy_ip=10.1.2.3; proxy_on_host=0; do_selftest=1
    range_url=http://10.9.9.9:9400; range_tenant=prod; range_token_file="$3"
    site_key=0xSITE; secret_file="$2"; smtp_host=smtp.example.org; smtp_port=2525
    smtp_user=mailer; smtp_from="necropsy <no-reply@example.org>"
    '"${1:-:}"'
    write_settings
  ' "$script" "$conf" "$work/ts" "$work/token"
}
check "the settings step runs"                  settings
env="$conf/necropsy.env" dep="$conf/deploy.env"
check "base url"                                grep -qx 'NECROPSY_BASE_URL=https://necropsy.example.org' "$env"
check "live domain"                             grep -qx 'NECROPSY_LIVE_DOMAIN=necropsy.example.org' "$env"
check "the site gets the range's address"       grep -qx 'NECROPSY_RANGE_URL=http://10.9.9.9:9400' "$env"
check "...and its token"                        grep -qx "NECROPSY_RANGE_TOKEN=$TOKEN" "$env"
check "no unix socket settings from the old design" fails grep -q '^NECROPSY_SPAWNER_SOCKET\|^NECROPSY_RANGE_DIR' "$env"
check "the deploy program gets the range url"   grep -qx 'RANGE_URL=http://10.9.9.9:9400' "$dep"
check "...and the tenant"                       grep -qx 'RANGE_TENANT=prod' "$dep"
check "...and tests the labs"                   grep -qx 'RANGE_SELFTEST=1' "$dep"
check "the token file is written root-only"     equals "$(stat -c %a "$conf/range.token")" 600
check "...holding the token, without whitespace" equals "$(cat "$conf/range.token")" "$TOKEN"
check "turnstile site key"                      grep -qx 'NECROPSY_TURNSTILE_SITE_KEY=0xSITE' "$env"
check "turnstile secret, read from the file"    grep -qx 'NECROPSY_TURNSTILE_SECRET=0xTHESECRET' "$env"
check "smtp port"                               grep -qx 'NECROPSY_SMTP_PORT=2525' "$env"
check "smtp password, from the environment"     grep -qx 'NECROPSY_SMTP_PASSWORD=p4ss' "$env"
check "smtp from is quoted"                     grep -qx 'NECROPSY_SMTP_FROM="necropsy <no-reply@example.org>"' "$env"
check "the env file stays private"              equals "$(stat -c %a "$env")" 600
check "the proxy is published for another host" grep -qx 'NECROPSY_BIND=0.0.0.0' "$dep"
check "and only that proxy is trusted"          grep -qx 'NECROPSY_EDGE_TRUSTED=10.1.2.3' "$dep"
check "the repository setting is untouched"     grep -qx 'NECROPSY_REPO=https://github.com/o/r' "$dep"
settings
check "running it again changes nothing"        equals "$(grep -c '^NECROPSY_LIVE_DOMAIN=' "$env")" 1
settings 'do_selftest=0'
check "--no-selftest turns the test off"        grep -qx 'RANGE_SELFTEST=0' "$dep"
settings 'proxy_on_host=1'
check "a proxy on the host publishes on loopback" grep -qx 'NECROPSY_BIND=127.0.0.1' "$dep"

# A site with no range gets no range settings at all.
conf2="$work/etc2"; mkdir -p "$conf2"
cp "$repo_root/.env.example" "$conf2/necropsy.env"; : > "$conf2/deploy.env"
NECROPSY_INSTALL_SOURCED=1 bash -c '
  . "$0"; CONF_DIR="$1"; dry=0; domain=a.example.org; proxy_on_host=1; do_selftest=1
  write_settings' "$script" "$conf2" 2>/dev/null
check "a site without a range sets no range url"  fails grep -q '^NECROPSY_RANGE_URL=\|^RANGE_URL=' "$conf2/necropsy.env" "$conf2/deploy.env"
check "...and writes no token file"             test ! -e "$conf2/range.token"

# ---- the check of the range VM -------------------------------------------------------

python3 "$here/fake-range.py" "$TOKEN" > "$work/port" &
fake_pid=$!
for _ in $(seq 1 50); do [ -s "$work/port" ] && break; sleep 0.1; done
port="$(cat "$work/port")"
range_check() { # url token-file do_deploy
  NECROPSY_INSTALL_SOURCED=1 bash -c '
    . "$0"; dry=0; range_url="$1"; range_tenant=prod; range_token_file="$2"; do_deploy="$3"
    check_range' "$script" "$@"
}
printf 'wrong-token-0123456789abcdef\n' > "$work/wrong"
check "a range that answers and accepts the token passes" range_check "http://127.0.0.1:$port" "$work/token" 1
check "a token the range rejects stops the install"       fails range_check "http://127.0.0.1:$port" "$work/wrong" 1
check "...and says which tenant's token to use"           bash -c "! out=\"\$(NECROPSY_INSTALL_SOURCED=1 bash -c '. \"$script\"; dry=0; range_url=http://127.0.0.1:$port; range_tenant=prod; range_token_file=$work/wrong; do_deploy=1; check_range' 2>&1)\"; echo \"\$out\" | grep -q 'prod tenant'"
check "a range that does not answer stops the install"    fails range_check "http://127.0.0.1:1" "$work/token" 1
check "...but only warns when not deploying"              range_check "http://127.0.0.1:1" "$work/token" 0
check "a site without a range skips the check"            bash -c 'NECROPSY_INSTALL_SOURCED=1; . "$0"; dry=0; range_url=""; check_range' "$script"

# ---- full dry runs --------------------------------------------------------------

out="$(dry "${ok_args[@]}" "${range_args[@]}" --turnstile-site-key 0xAB --turnstile-secret-file "$work/secret" --smtp-host smtp.example.org 2>&1)"
check "a dry run succeeds"                       equals "$?" 0
check "it installs the deploy program"           grep -q 'install-system.sh --user deploy --repo https://github.com/o/r --branch main' <<<"$out"
check "it restricts the port to the proxy"       grep -q "restricting the site's port to 10.0.0.5" <<<"$out"
check "it checks the range"                      grep -q 'check that http://10.9.9.9:9400 answers' <<<"$out"
check "it writes the range token"                grep -q 'range.token (mode 600)' <<<"$out"
check "it deploys"                               grep -q 'necropsy-deploy deploy' <<<"$out"
check "it prints the proxy block"                grep -q '@necropsy host a.example.org \*.a.example.org' <<<"$out"
check "it says nothing was changed"              grep -q 'Dry run: nothing was changed' <<<"$out"
check "it builds no spawner here: the range does that" fails grep -q 'cargo build\|necropsy-spawner' <<<"$out"
check "it installs no spawner service"           fails grep -q 'spawner.service' <<<"$out"
check "the secret never appears in the output"   fails grep -q '^secret$' <<<"$out"
check "the token never appears in the output"    fails grep -q "$TOKEN" <<<"$out"

out="$(dry "${ok_args[@]}" 2>&1)"
check "a dry run without a range warns that labs are hidden" grep -q 'container puzzles stay hidden' <<<"$out"
out="$(dry --domain a.example.org --repo https://github.com/o/r --proxy-on-host --no-deploy --no-firewall --branch dev 2>&1)"
check "a proxy on the host needs no firewall rule" fails grep -q 'restricting the site' <<<"$out"
check "--no-deploy skips the deploy"             grep -q 'skipping the deploy' <<<"$out"
check "the dev VM builds its own branch"         grep -q -- '--branch dev' <<<"$out"

check "a dry run changed nothing here" \
  bash -c '[ ! -e /usr/local/bin/necropsy-deploy ] && [ ! -e /etc/systemd/system/necropsy-firewall.service ] && [ ! -e /etc/necropsy/range.token ]'

echo
if [ "$failures" -eq 0 ]; then echo "all passed"; else echo "$failures failed"; exit 1; fi
