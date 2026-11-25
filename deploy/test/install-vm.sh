#!/usr/bin/env bash
# Tests deploy/install-vm.sh: its argument checks, the settings editor, the
# sizing, the two environments, and a full --dry-run. Nothing here installs
# anything.
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
# NECROPSY_INSTALL_SOURCED is set), in its own shell, since the script sets -e.
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

check "two thirds of 51 go to production"  equals "$(fn 'split_instances 51 prod')" 34
check "and the rest to dev"                equals "$(fn 'split_instances 51 dev')" 17
check "the split of 19"                    equals "$(fn 'echo $(split_instances 19 prod) $(split_instances 19 dev)')" "13 6"
check "a tiny total still gives each 2"    equals "$(fn 'echo $(split_instances 2 prod) $(split_instances 2 dev)')" "2 2"

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

# ---- the files the installers and the deploy program rewrite -----------------------

check "images.toml has the lines the installers rewrite" \
  bash -c 'grep -q "^max_instances *=" "$0" && grep -q "^cpus *=" "$0" && grep -q "necropsy-range/[a-z0-9-]*:1\"" "$0"' "$repo_root/range/images.toml"
check "the spawner unit is a template" grep -q '@NAME@' "$repo_root/deploy/range/necropsy-spawner.service.in"
check "...with no fixed stack name left in its paths" \
  bash -c '! grep -v "^#" "$0" | grep -q "/run/necropsy\|/etc/necropsy"' "$repo_root/deploy/range/necropsy-spawner.service.in"

# ---- argument checks ------------------------------------------------------------

ok_args=(--domain a.example.org --proxy-ip 10.0.0.5 --repo https://github.com/o/r)
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
check "a non-number max is refused"             fails dry "${ok_args[@]}" --max-instances many
check "a site key without a secret is refused"  fails dry "${ok_args[@]}" --turnstile-site-key 0xAB
echo secret > "$work/secret"
check "a secret without a site key is refused"  fails dry "${ok_args[@]}" --turnstile-secret-file "$work/secret"
check "an unknown option is refused"            fails dry "${ok_args[@]}" --frobnicate
check "--help works"                            bash "$script" --help
check "a dev domain alone is fine"              dry "${ok_args[@]}" --dev-domain dev.example.org
check "a dev domain equal to production is refused"   fails dry "${ok_args[@]}" --dev-domain a.example.org
check "a bad dev domain is refused"             fails dry "${ok_args[@]}" --dev-domain 'x y'
check "dev and production cannot share a deploy user" fails dry "${ok_args[@]}" --dev-domain dev.example.org --dev-deploy-user deploy
check "root as the dev user is refused"         fails dry "${ok_args[@]}" --dev-domain dev.example.org --dev-deploy-user root
check "a bad dev branch is refused"             fails dry "${ok_args[@]}" --dev-domain dev.example.org --dev-branch '-x'
check "--dev-max-instances needs a dev domain"  fails dry "${ok_args[@]}" --dev-max-instances 5

# ---- the two environments ---------------------------------------------------------

envinfo() { # args..., then a snippet that prints e_* after select_env
  NECROPSY_INSTALL_SOURCED=1 bash -c '
    . "$0"; total_mb=8192; cpus_n=4
    domain=a.example.org; deploy_user=deploy; branch=main
    dev_domain="$DEV"; dev_user=deploy-dev; dev_branch=dev
    select_env "$1"; echo "$e_name $e_user $e_branch $e_domain $e_port $e_subnet $e_max $e_build $e_restart"
  ' "$script" "$@"
}
check "production alone has the whole range" \
  equals "$(DEV= envinfo prod)" "necropsy deploy main a.example.org 8080 172.30.30.0/24 51 1 "
check "with dev, production has two thirds" \
  equals "$(DEV=dev.example.org envinfo prod)" "necropsy deploy main a.example.org 8080 172.30.30.0/24 34 1 necropsy-dev-spawner"
check "dev has its own name, user, branch, domain, port and subnet" \
  equals "$(DEV=dev.example.org envinfo dev)" "necropsy-dev deploy-dev dev dev.example.org 8081 172.30.31.0/24 17 0 "
check "only production builds the spawner program" \
  bash -c '[ "$(echo "$0" | cut -d" " -f8)" = 1 ] && [ "$(echo "$1" | cut -d" " -f8)" = 0 ]' "$(DEV=dev.example.org envinfo prod)" "$(DEV=dev.example.org envinfo dev)"

# ---- the real-mode steps, against a temporary /etc ---------------------------------
#
# Everything that would change the machine goes through `run`, which is stubbed
# out here; the files the script writes itself go to a temporary directory.

conf="$work/etc"
for n in necropsy necropsy-dev; do
  mkdir -p "$conf/$n"
  cp "$repo_root/.env.example" "$conf/$n/necropsy.env"
  chmod 600 "$conf/$n/necropsy.env"
  printf '#NECROPSY_BIND=127.0.0.1\n#NECROPSY_PORT=8080\n#NECROPSY_EDGE_TRUSTED=172.30.30.1,127.0.0.1\nNECROPSY_REPO=https://github.com/o/r\n' > "$conf/$n/deploy.env"
done
echo '0xTHESECRET' > "$work/ts"

settings() { # prod | dev
  NECROPSY_INSTALL_SOURCED=1 NECROPSY_SMTP_PASSWORD='p4ss' bash -c '
    . "$0"; CONF_ROOT="$1"; dry=0; run() { :; }
    total_mb=8192; cpus_n=4
    domain=necropsy.example.org; deploy_user=deploy; branch=main
    dev_domain=dev.example.org; dev_user=deploy-dev; dev_branch=dev
    proxy_ip=10.1.2.3; proxy_on_host=0; do_selftest=1
    site_key=0xSITE; secret_file="$2"; smtp_host=smtp.example.org; smtp_port=2525
    smtp_user=mailer; smtp_from="necropsy <no-reply@example.org>"
    select_env "$3"; write_settings
  ' "$script" "$conf" "$work/ts" "$1"
}
check "production's settings step runs"          settings prod
penv="$conf/necropsy/necropsy.env" pdep="$conf/necropsy/deploy.env"
check "base url"                                grep -qx 'NECROPSY_BASE_URL=https://necropsy.example.org' "$penv"
check "live domain"                             grep -qx 'NECROPSY_LIVE_DOMAIN=necropsy.example.org' "$penv"
check "the spawner socket is the container's path" grep -qx 'NECROPSY_SPAWNER_SOCKET=/run/necropsy/spawner.sock' "$penv"
check "range dir"                               grep -qx 'NECROPSY_RANGE_DIR=/run/necropsy/range' "$penv"
check "turnstile site key"                      grep -qx 'NECROPSY_TURNSTILE_SITE_KEY=0xSITE' "$penv"
check "turnstile secret, read from the file"    grep -qx 'NECROPSY_TURNSTILE_SECRET=0xTHESECRET' "$penv"
check "smtp port"                               grep -qx 'NECROPSY_SMTP_PORT=2525' "$penv"
check "smtp password, from the environment"     grep -qx 'NECROPSY_SMTP_PASSWORD=p4ss' "$penv"
check "smtp from is quoted"                     grep -qx 'NECROPSY_SMTP_FROM="necropsy <no-reply@example.org>"' "$penv"
check "the env file stays private"              equals "$(stat -c %a "$penv")" 600
check "the proxy is published for another host" grep -qx 'NECROPSY_BIND=0.0.0.0' "$pdep"
check "and only that proxy is trusted"          grep -qx 'NECROPSY_EDGE_TRUSTED=10.1.2.3' "$pdep"
check "production's port"                       grep -qx 'NECROPSY_PORT=8080' "$pdep"
check "production's subnet"                     grep -qx 'NECROPSY_SUBNET=172.30.30.0/24' "$pdep"
check "production's spawner unit"               grep -qx 'RANGE_UNIT=necropsy-spawner' "$pdep"
check "production's lab limit (two thirds)"     grep -qx 'RANGE_MAX_INSTANCES=34' "$pdep"
check "production builds the spawner program"   grep -qx 'RANGE_BUILD_SPAWNER=1' "$pdep"
check "and restarts dev's spawner when it changes" grep -qx 'RANGE_RESTART_UNITS=necropsy-dev-spawner' "$pdep"
check "labs are tested on deploy"               grep -qx 'RANGE_SELFTEST=1' "$pdep"
check "the repository setting is untouched"     grep -qx 'NECROPSY_REPO=https://github.com/o/r' "$pdep"
check "no cpu cap on a 4 vCPU VM"               fails grep -q '^RANGE_CPUS_CAP' "$pdep"

check "dev's settings step runs"                settings dev
denv="$conf/necropsy-dev/necropsy.env" ddep="$conf/necropsy-dev/deploy.env"
check "dev has its own base url"                grep -qx 'NECROPSY_BASE_URL=https://dev.example.org' "$denv"
check "...and live domain"                      grep -qx 'NECROPSY_LIVE_DOMAIN=dev.example.org' "$denv"
check "dev's port"                              grep -qx 'NECROPSY_PORT=8081' "$ddep"
check "dev's subnet"                            grep -qx 'NECROPSY_SUBNET=172.30.31.0/24' "$ddep"
check "dev's spawner unit"                      grep -qx 'RANGE_UNIT=necropsy-dev-spawner' "$ddep"
check "dev's lab limit (the rest)"              grep -qx 'RANGE_MAX_INSTANCES=17' "$ddep"
check "dev never builds the spawner program"    grep -qx 'RANGE_BUILD_SPAWNER=0' "$ddep"
check "production's settings were not touched by dev's" grep -qx 'NECROPSY_BASE_URL=https://necropsy.example.org' "$penv"
settings prod
check "running it again changes nothing"        equals "$(grep -c '^NECROPSY_LIVE_DOMAIN=' "$penv")" 1

spawner_step() { # prod | dev, vcpus
  NECROPSY_INSTALL_SOURCED=1 bash -c '
    . "$0"; CONF_ROOT="$1"; SYSTEMD_DIR="$2"; repo_root="$3"; dry=0
    run() { :; }; wait_for_spawner() { :; }
    total_mb=8192; cpus_n="$5"
    domain=a.example.org; deploy_user=deploy; branch=main
    dev_domain=dev.example.org; dev_user=deploy-dev; dev_branch=dev
    select_env "$4"; install_env_spawner
  ' "$script" "$conf" "$work/units" "$repo_root" "$@"
}
mkdir -p "$work/units"
rm -f "$conf"/necropsy/images.toml "$conf"/necropsy-dev/images.toml
check "production's spawner step runs"          spawner_step prod 4
check "its unit is named for it"                test -f "$work/units/necropsy-spawner.service"
check "...with its socket"                      grep -q 'SPAWNER_SOCKET=/run/necropsy/spawner.sock' "$work/units/necropsy-spawner.service"
check "...and its allowlist"                    grep -q 'SPAWNER_IMAGES=/etc/necropsy/images.toml' "$work/units/necropsy-spawner.service"
check "no template placeholder is left"         fails grep -q '@NAME@' "$work/units/necropsy-spawner.service"
check "production's starting allowlist has its limit" grep -qx 'max_instances = 34' "$conf/necropsy/images.toml"
check "...and images of its own"                grep -q 'necropsy-range/necropsy/webshell-lab:initial' "$conf/necropsy/images.toml"
check "dev's spawner step runs"                 spawner_step dev 4
check "dev's unit is its own"                   grep -q 'SPAWNER_SOCKET=/run/necropsy-dev/spawner.sock' "$work/units/necropsy-dev-spawner.service"
check "dev's runtime directory is its own"      grep -qx 'RuntimeDirectory=necropsy-dev' "$work/units/necropsy-dev-spawner.service"
check "dev's starting allowlist has its limit"  grep -qx 'max_instances = 17' "$conf/necropsy-dev/images.toml"
check "...and images of its own"                grep -q 'necropsy-range/necropsy-dev/webshell-lab:initial' "$conf/necropsy-dev/images.toml"
echo "# written by a deploy" >> "$conf/necropsy/images.toml"
spawner_step prod 4
check "an allowlist a deploy wrote is kept"     grep -q '^# written by a deploy' "$conf/necropsy/images.toml"
rm -f "$conf/necropsy-dev/images.toml"
spawner_step dev 2
check "a small VM caps each lab's cpu"          grep -qx 'cpus = 0.25' "$conf/necropsy-dev/images.toml"

# ---- a full dry run -------------------------------------------------------------

out="$(dry --domain necropsy.example.org --proxy-ip 10.1.2.3 --repo https://github.com/o/r \
        --turnstile-site-key 0xAB --turnstile-secret-file "$work/secret" \
        --smtp-host smtp.example.org --max-instances 7 2>&1)"
rc=$?
check "a dry run succeeds"                       equals "$rc" 0
check "it builds the spawner in a container"     grep -q 'docker run --rm .*-alpine' <<<"$out"
check "it installs and restarts the spawner"     grep -q 'systemctl restart necropsy-spawner' <<<"$out"
check "it installs the deploy program"           grep -q 'install-system.sh --name necropsy --user deploy --repo https://github.com/o/r --branch main' <<<"$out"
check "it restricts the ports to the proxy"      grep -q "restricting the sites' ports to 10.1.2.3" <<<"$out"
check "it deploys"                               grep -q 'necropsy-deploy deploy' <<<"$out"
check "it prints the proxy block"                grep -q '@necropsy host necropsy.example.org \*.necropsy.example.org' <<<"$out"
check "it says nothing was changed"              grep -q 'Dry run: nothing was changed' <<<"$out"
check "there is no dev stack without --dev-domain" fails grep -q 'necropsy-dev' <<<"$out"
check "the secret never appears in the output"   fails grep -q '^secret$' <<<"$out"

out="$(dry --domain necropsy.example.org --proxy-ip 10.1.2.3 --repo https://github.com/o/r \
        --dev-domain dev.example.org --dev-max-instances 4 2>&1)"
check "a dry run with a dev stack succeeds"      equals "$?" 0
check "dev gets its own deploy program"          grep -q 'install-system.sh --name necropsy-dev --user deploy-dev --repo https://github.com/o/r --branch dev' <<<"$out"
check "dev gets its own spawner"                 grep -q 'systemctl restart necropsy-dev-spawner' <<<"$out"
check "production still gets its own"            grep -q 'install-system.sh --name necropsy --user deploy --repo' <<<"$out"
check "dev is deployed from its branch"          grep -q 'necropsy-dev-deploy deploy <newest commit of dev' <<<"$out"
check "both proxy blocks are printed"            bash -c 'grep -q "@necropsy host necropsy.example.org" <<<"$0" && grep -q "@necropsy-dev host dev.example.org \*.dev.example.org" <<<"$0"' "$out"
check "the dev site is on its own port"          grep -q ':8081 {' <<<"$out"
check "one firewall rule covers both"            equals "$(grep -c 'restricting the sites' <<<"$out")" 1
check "the deploy users are told apart"          grep -q 'deploy for production, deploy-dev for dev' <<<"$out"

out="$(dry --domain necropsy.example.org --repo https://github.com/o/r --proxy-on-host --no-deploy --no-firewall 2>&1)"
check "a proxy on the host needs no firewall rule" fails grep -q 'restricting the sites' <<<"$out"
check "--no-deploy skips the deploy"             grep -q 'skipping the deploy' <<<"$out"
check "--no-deploy still installs everything else" grep -q 'install-system.sh' <<<"$out"

check "a dry run changed nothing here" \
  bash -c '[ ! -e /usr/local/bin/necropsy-spawner ] && [ ! -e /etc/systemd/system/necropsy-firewall.service ] && [ ! -e /etc/systemd/system/necropsy-dev-spawner.service ]'

echo
if [ "$failures" -eq 0 ]; then echo "all passed"; else echo "$failures failed"; exit 1; fi
