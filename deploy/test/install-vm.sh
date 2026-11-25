#!/usr/bin/env bash
# Tests deploy/install-vm.sh: its argument checks, the settings editor, the
# sizing, and a full --dry-run. Nothing here installs anything.
#
#   deploy/test/install-vm.sh

set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$here/../install-vm.sh"
repo_root="$(cd "$here/../.." && pwd)"
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

# Run a snippet with the script's functions loaded (it does not run main when
# sourced), in its own shell, since the script sets -e.
fn() { NECROPSY_INSTALL_SOURCED=1 bash -c '. "$0"; '"$1" "$script" "${@:2}"; }
# The script itself, as a dry run.
dry() { bash "$script" --dry-run "$@"; }

# ---- sizing: the "set max_instances to" column of docs/range.md ------------------

check "1 GB holds the minimum of 2"       equals "$(fn 'suggest_max_instances 1024')" 2
check "2 GB"                              equals "$(fn 'suggest_max_instances 2048')" 3
check "4 GB (the doc says about 20)"      equals "$(fn 'suggest_max_instances 4096')" 19
check "8 GB (about 50)"                   equals "$(fn 'suggest_max_instances 8192')" 51
check "16 GB (about 110)"                 equals "$(fn 'suggest_max_instances 16384')" 114
check "32 GB (about 230)"                 equals "$(fn 'suggest_max_instances 32768')" 240

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

# ---- cpu cap --------------------------------------------------------------------

printf '[images.a]\ncpus = 0.5\n[images.b]\ncpus = 0.1\n[images.c]\ncpus = 2\n' > "$work/img"
fn 'cap_cpus "$1" 0.25' "$work/img"
check "a higher cpu limit is lowered"            equals "$(sed -n 's/^cpus = //p' "$work/img" | tr '\n' ' ')" "0.25 0.1 0.25 "

# ---- the images file the installer edits ------------------------------------------

check "images.toml has the lines the installer rewrites" \
  bash -c 'grep -q "^max_instances *=" "$0" && grep -q "^cpus *=" "$0"' "$repo_root/range/images.toml"

# ---- argument checks ------------------------------------------------------------

check "no arguments is refused"                 fails dry
check "a domain alone is refused (no proxy)"    fails dry --domain necropsy.example.org
check "a bad domain is refused"                 fails dry --domain 'Not A Domain' --proxy-ip 10.0.0.5
check "a domain with a path is refused"         fails dry --domain 'a.example.org/x' --proxy-ip 10.0.0.5
check "a bad proxy address is refused"          fails dry --domain a.example.org --proxy-ip 'nope'
check "a list of proxies is refused"            fails dry --domain a.example.org --proxy-ip 10.0.0.5,10.0.0.6
check "both proxy options are refused"          fails dry --domain a.example.org --proxy-ip 10.0.0.5 --proxy-on-host
check "root as the deploy user is refused"      fails dry --domain a.example.org --proxy-ip 10.0.0.5 --deploy-user root
check "a bad branch is refused"                 fails dry --domain a.example.org --proxy-ip 10.0.0.5 --branch '../x'
check "an http repo is refused"                 fails dry --domain a.example.org --proxy-ip 10.0.0.5 --repo http://github.com/o/r
check "a non-number max is refused"             fails dry --domain a.example.org --proxy-ip 10.0.0.5 --max-instances many
check "a site key without a secret is refused"  fails dry --domain a.example.org --proxy-ip 10.0.0.5 --turnstile-site-key 0xAB
echo secret > "$work/secret"
check "a secret without a site key is refused"  fails dry --domain a.example.org --proxy-ip 10.0.0.5 --turnstile-secret-file "$work/secret"
check "an unknown option is refused"            fails dry --domain a.example.org --proxy-ip 10.0.0.5 --frobnicate
check "--help works"                            bash "$script" --help

# ---- a full dry run -------------------------------------------------------------

out="$(dry --domain necropsy.example.org --proxy-ip 10.1.2.3 --repo https://github.com/o/r \
        --turnstile-site-key 0xAB --turnstile-secret-file "$work/secret" \
        --smtp-host smtp.example.org --max-instances 7 2>&1)"
rc=$?
check "a dry run succeeds"                       equals "$rc" 0
check "it builds the spawner in a container"     grep -q 'docker run --rm .*-alpine' <<<"$out"
check "it builds the lab images"                 grep -q 'range/build.sh' <<<"$out"
check "it installs and restarts the spawner"     grep -q 'systemctl restart necropsy-spawner' <<<"$out"
check "it tests a real lab"                      grep -q 'testing a real lab' <<<"$out"
check "it installs the deploy program"           grep -q 'install-system.sh --user deploy --repo https://github.com/o/r --branch main' <<<"$out"
check "it restricts the port to the proxy"       grep -q 'restricting the site.s port to 10.1.2.3' <<<"$out"
check "it deploys"                               grep -q 'necropsy-deploy deploy' <<<"$out"
check "it prints the proxy block"                grep -q '@necropsy host necropsy.example.org \*.necropsy.example.org' <<<"$out"
check "it says nothing was changed"              grep -q 'Dry run: nothing was changed' <<<"$out"
check "the secret never appears in the output"   fails grep -q 'secret' <<<"$(grep -v 'turnstile\|Turnstile\|TURNSTILE' <<<"$out")"
check "and not the secret's value either"        fails grep -q '^secret$' <<<"$out"

out="$(dry --domain necropsy.example.org --repo https://github.com/o/r --proxy-on-host --no-deploy --no-selftest --no-firewall 2>&1)"
check "a proxy on the host needs no firewall rule" fails grep -q 'restricting the site' <<<"$out"
check "--no-deploy skips the deploy"             grep -q 'skipping the deploy' <<<"$out"
check "--no-selftest skips the lab test"         fails grep -q 'testing a real lab' <<<"$out"

check "a dry run changed nothing here" \
  bash -c '[ ! -e /usr/local/bin/necropsy-spawner ] && [ ! -e /etc/systemd/system/necropsy-firewall.service ]'

# ---- the real-mode steps, against a temporary /etc/necropsy -----------------------
#
# Everything that would change the machine goes through `run`, which is stubbed
# out here; the files the script writes itself go to a temporary directory.

conf="$work/etc"
mkdir -p "$conf"
cp "$repo_root/.env.example" "$conf/necropsy.env"
chmod 600 "$conf/necropsy.env"
printf '#NECROPSY_BIND=127.0.0.1\n#NECROPSY_EDGE_TRUSTED=172.30.30.1,127.0.0.1\nNECROPSY_REPO=https://github.com/o/r\n' > "$conf/deploy.env"
echo '0xTHESECRET' > "$work/ts"

settings() { # extra shell, run after the settings step
  NECROPSY_INSTALL_SOURCED=1 NECROPSY_SMTP_PASSWORD='p4ss' bash -c '
    . "$0"; CONF_DIR="$1"; dry=0; run() { :; }
    domain=necropsy.example.org; proxy_ip=10.1.2.3; proxy_on_host=0
    site_key=0xSITE; secret_file="$2"; smtp_host=smtp.example.org; smtp_port=2525
    smtp_user=mailer; smtp_from="necropsy <no-reply@example.org>"
    write_settings
  ' "$script" "$conf" "$work/ts"
}
check "the settings step runs"                  settings
envf="$conf/necropsy.env"
check "base url"                                grep -qx 'NECROPSY_BASE_URL=https://necropsy.example.org' "$envf"
check "live domain"                             grep -qx 'NECROPSY_LIVE_DOMAIN=necropsy.example.org' "$envf"
check "spawner socket"                          grep -qx 'NECROPSY_SPAWNER_SOCKET=/run/necropsy/spawner.sock' "$envf"
check "range dir"                               grep -qx 'NECROPSY_RANGE_DIR=/run/necropsy/range' "$envf"
check "turnstile site key"                      grep -qx 'NECROPSY_TURNSTILE_SITE_KEY=0xSITE' "$envf"
check "turnstile secret, read from the file"    grep -qx 'NECROPSY_TURNSTILE_SECRET=0xTHESECRET' "$envf"
check "smtp host and port"                      grep -qx 'NECROPSY_SMTP_PORT=2525' "$envf"
check "smtp password, from the environment"     grep -qx 'NECROPSY_SMTP_PASSWORD=p4ss' "$envf"
check "smtp from is quoted"                     grep -qx 'NECROPSY_SMTP_FROM="necropsy <no-reply@example.org>"' "$envf"
check "the env file stays private"              equals "$(stat -c %a "$envf")" 600
check "the proxy is published for another host" grep -qx 'NECROPSY_BIND=0.0.0.0' "$conf/deploy.env"
check "and only that proxy is trusted"          grep -qx 'NECROPSY_EDGE_TRUSTED=10.1.2.3' "$conf/deploy.env"
check "the repository setting is untouched"     grep -qx 'NECROPSY_REPO=https://github.com/o/r' "$conf/deploy.env"
settings
check "running it again changes nothing"        equals "$(grep -c '^NECROPSY_LIVE_DOMAIN=' "$envf")" 1

images() { # max_instances flag, vcpus, memory
  NECROPSY_INSTALL_SOURCED=1 bash -c '
    . "$0"; CONF_DIR="$1"; dry=0; run() { :; }
    repo_root="$2"; max_instances="$3"; cpus_n="$4"; total_mb="$5"
    install_images
  ' "$script" "$conf" "$repo_root" "$@"
}
img="$conf/images.toml"
check "the allowlist is installed"              images "" 4 8192
check "sized to the memory"                     grep -qx 'max_instances = 51' "$img"
check "a bigger VM keeps the lab's cpu limit"   grep -qx 'cpus = 0.5' "$img"
rm -f "$img"
images "" 2 4096
check "a small VM caps each lab's cpu"          grep -qx 'cpus = 0.25' "$img"
check "...and is sized to its memory"           grep -qx 'max_instances = 19' "$img"
images 33 4 8192
check "an explicit max wins"                    grep -qx 'max_instances = 33' "$img"
images "" 4 8192
check "a later run keeps the earlier max"       grep -qx 'max_instances = 33' "$img"
check "the written file still lists the labs"       grep -q "^\[images.webshell-lab\]" "$img"

echo
if [ "$failures" -eq 0 ]; then echo "all passed"; else echo "$failures failed"; exit 1; fi
