#!/usr/bin/env bash
# Tests deploy/install-range.sh: its argument checks, the limits it gives each
# tenant, the tokens and tenants file it writes, the firewall it sets up (the
# generated script is run against a fake iptables), and a full --dry-run. Nothing
# here installs anything.
#
#   deploy/test/install-range.sh

set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$here/../install-range.sh"
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

fn() { NECROPSY_INSTALL_SOURCED=1 bash -c '. "$0"; '"$1" "$script" "${@:2}"; }
dry() { bash "$script" --dry-run "$@"; }

# ---- limits ---------------------------------------------------------------------

# The same figures as docs/range.md: 143 MB planned per lab, 10% over.
check "a whole 15 GB VM for one tenant"           equals "$(fn 'tenant_limits 15063 1 1')" "115 14720"
check "two thirds of it"                          equals "$(fn 'tenant_limits 15063 2 3')" "77 9856"
check "one third of it"                           equals "$(fn 'tenant_limits 15063 1 3')" "38 4864"
check "the budget is the labs' limits together"   equals "$(fn 'read -r n b <<<"$(tenant_limits 4000 1 1)"; echo $((b / n))')" 128
check "a tiny share still gets two labs"          equals "$(fn 'tenant_limits 600 1 9')" "2 256"
check "the shares of a 3 GB VM stay inside it" \
  fn 'cap=3000; tot=0; for w in 2 1; do read -r n b <<<"$(tenant_limits $cap $w 3)"; tot=$((tot + b)); done; [ "$tot" -le "$cap" ]'

# ---- tokens ---------------------------------------------------------------------

check "a token hash is sha256 hex"                equals "$(fn 'token_hash x')" "2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881"
tok="$(fn new_token)"
check "a new token is 64 hex digits"              bash -c '[[ "$0" =~ ^[0-9a-f]{64}$ ]]' "$tok"
check "two tokens differ"                         bash -c '[ "$0" != "$1" ]' "$tok" "$(fn new_token)"

# ---- argument checks ------------------------------------------------------------

ok_args=(--listen 10.9.9.9 --tenant prod:10.0.0.21 --tenant dev:10.0.0.22)
check "no arguments is refused"                   fails dry
check "--listen is required"                      fails dry --tenant prod:10.0.0.21
check "a tenant is required"                      fails dry --listen 10.9.9.9
check "a good command line is fine"               dry "${ok_args[@]}"
check "a hostname is not an address to listen on" fails dry --listen range.local --tenant prod:10.0.0.21
check "an address out of range is refused"        fails dry --listen 10.9.9.999 --tenant prod:10.0.0.21
check "a tenant needs an address"                 fails dry --listen 10.9.9.9 --tenant prod
check "a tenant's address must be one address"    fails dry --listen 10.9.9.9 --tenant prod:10.0.0.0/24
check "a bad tenant name is refused"              fails dry --listen 10.9.9.9 --tenant Prod:10.0.0.21
check "a tenant listed twice is refused"          fails dry --listen 10.9.9.9 --tenant prod:10.0.0.21 --tenant prod:10.0.0.22
check "a share must be a number"                  fails dry --listen 10.9.9.9 --tenant prod:10.0.0.21:lots
check "a share of 0 is refused"                   fails dry --listen 10.9.9.9 --tenant prod:10.0.0.21:0
check "a port below 1024 is refused"              fails dry "${ok_args[@]}" --port 80
check "a port above 65535 is refused"             fails dry "${ok_args[@]}" --port 70000
check "a tiny reserve is refused"                 fails dry "${ok_args[@]}" --reserve-mb 10
check "an unknown option is refused"              fails dry "${ok_args[@]}" --frobnicate
check "an http repository is refused"             fails dry "${ok_args[@]}" --repo http://github.com/o/r
check "a bad branch is refused"                   fails dry "${ok_args[@]}" --repo https://github.com/o/r --branch '../x'
check "a branch that looks like an option is refused" fails dry "${ok_args[@]}" --repo https://github.com/o/r --branch '-x'
check "--help works"                              bash "$script" --help

# ---- the real-mode steps, against a temporary /etc -----------------------------------
#
# Everything that would change the machine goes through `run`, which is stubbed
# out here; the files the script writes itself go to a temporary directory.

conf="$work/etc"; mkdir -p "$conf/tokens"
write_tenants_with() { # extra shell, run first
  NECROPSY_INSTALL_SOURCED=1 bash -c '
    . "$0"; CONF_DIR="$1"; dry=0; run() { :; }
    listen=10.9.9.9; port=9400; capacity_mb=15000
    t_names=(prod dev); t_ips=(10.0.0.21 10.0.0.22); t_weights=(2 1)
    '"${1:-:}"'
    write_tenants' "$script" "$conf"
}
check "the tenants step runs"                     write_tenants_with
check "each tenant gets a token file"             test -s "$conf/tokens/prod" -a -s "$conf/tokens/dev"
check "...readable by root only"                  bash -c '[ "$(stat -c %a "$0/tokens/prod")" = 600 ] && [ "$(stat -c %a "$0/tokens/dev")" = 600 ]' "$conf"
check "...with different tokens"                  bash -c '[ "$(cat "$0/tokens/prod")" != "$(cat "$0/tokens/dev")" ]' "$conf"
hash_prod="$(printf '%s' "$(tr -d '[:space:]' < "$conf/tokens/prod")" | sha256sum | cut -d' ' -f1)"
check "the tenants file holds the hash of a token" grep -qx "token_sha256 = \"$hash_prod\"" "$conf/tenants.toml"
check "...and never the token itself"             bash -c '! grep -q "$(cat "$0/tokens/prod")" "$0/tenants.toml"' "$conf"
check "the tenants file is not world-readable"    equals "$(stat -c %a "$conf/tenants.toml")" 640
check "the first tenant gets its share"           bash -c 'grep -A2 "^\[tenants.prod\]" "$0/tenants.toml" | grep -qx "max_instances = 76"' "$conf"
check "the second gets its own"                   bash -c 'grep -A2 "^\[tenants.dev\]" "$0/tenants.toml" | grep -qx "max_instances = 38"' "$conf"
check "the spawner is told where to listen"       grep -qx 'SPAWNER_LISTEN=10.9.9.9:9400' "$conf/spawner.env"
cp "$conf/tenants.toml" "$work/tenants.before"; cp "$conf/tokens/prod" "$work/prod.before"
write_tenants_with
check "running it again keeps the tokens"         cmp -s "$conf/tokens/prod" "$work/prod.before"
check "...and the tenants file the same"          cmp -s "$conf/tenants.toml" "$work/tenants.before"
write_tenants_with 'capacity_mb=6000'
check "a smaller VM changes the limits"           bash -c '! cmp -s "$0/tenants.toml" "$1"' "$conf" "$work/tenants.before"
check "...but not the tokens"                     cmp -s "$conf/tokens/prod" "$work/prod.before"
printf 'token-set-by-hand-0123456789abcdef\n' > "$conf/tokens/dev"
write_tenants_with
check "a token the admin chose is used"           grep -qx "token_sha256 = \"$(printf '%s' 'token-set-by-hand-0123456789abcdef' | sha256sum | cut -d' ' -f1)\"" "$conf/tenants.toml"

# The generated file is the format the spawner reads: run the spawner's own parser on it.
check "the spawner's own parser accepts the tenants file" \
  bash -c 'cd "$0" && SPAWNER_CHECK_TENANTS="$1" cargo test -q -p necropsy-spawner --bin necropsy-spawner generated_tenants_file 2>&1 | grep -q "1 passed"' "$repo_root" "$conf/tenants.toml"

# ---- the firewall ----------------------------------------------------------------

fw_dir="$work/fw"; mkdir -p "$fw_dir/lib" "$fw_dir/systemd" "$fw_dir/bin"
NECROPSY_INSTALL_SOURCED=1 bash -c '
  . "$0"; LIB_DIR="$1/lib"; SYSTEMD_DIR="$1/systemd"; dry=0; run() { :; }
  port=9500; do_firewall=1; listen=10.9.9.9; t_ips=(10.0.0.21 10.0.0.22)
  install_firewall' "$script" "$fw_dir" 2>/dev/null
check "the firewall script is written"            test -x "$fw_dir/lib/firewall.sh"
check "its unit runs it with the port and the sites" grep -qx "ExecStart=$fw_dir/lib/firewall.sh 9500 10.0.0.21 10.0.0.22" "$fw_dir/systemd/necropsy-range-firewall.service"
check "it starts before the spawner"              grep -qx 'Before=necropsy-spawner.service' "$fw_dir/systemd/necropsy-range-firewall.service"
# A fake iptables that logs what it is asked, and has an old jump rule to remove.
cat > "$fw_dir/bin/iptables" <<'IPT'
#!/bin/sh
echo "$@" >> "$FW_LOG"
if [ "$1" = -S ] && [ "$2" = INPUT ]; then
  echo "-P INPUT ACCEPT"
  echo "-A INPUT -p tcp -m tcp --dport 9400 -j NECROPSY-RANGE"
fi
exit 0
IPT
chmod +x "$fw_dir/bin/iptables"
FW_LOG="$fw_dir/log" PATH="$fw_dir/bin:$PATH" "$fw_dir/lib/firewall.sh" 9500 10.0.0.21 10.0.0.22
log="$(cat "$fw_dir/log")"
check "it makes a chain of its own and empties it" bash -c 'grep -qx -- "-N NECROPSY-RANGE" <<<"$0" && grep -qx -- "-F NECROPSY-RANGE" <<<"$0"' "$log"
check "it lets loopback in"                       grep -qx -- '-A NECROPSY-RANGE -i lo -j ACCEPT' <<<"$log"
check "it lets each site in"                      bash -c 'grep -qx -- "-A NECROPSY-RANGE -s 10.0.0.21 -j ACCEPT" <<<"$0" && grep -qx -- "-A NECROPSY-RANGE -s 10.0.0.22 -j ACCEPT" <<<"$0"' "$log"
check "it drops everyone else, last"              equals "$(grep -- '-A NECROPSY-RANGE' <<<"$log" | tail -n 1)" "-A NECROPSY-RANGE -j DROP"
check "it removes an earlier jump (the port may have changed)" grep -qx -- '-D INPUT -p tcp -m tcp --dport 9400 -j NECROPSY-RANGE' <<<"$log"
check "and jumps to the chain for the current port" grep -qx -- '-I INPUT -p tcp --dport 9500 -j NECROPSY-RANGE' <<<"$log"
check "the jump is added after the old one is removed" bash -c 'a=$(grep -n -- "-D INPUT" <<<"$0" | cut -d: -f1); b=$(grep -n -- "-I INPUT" <<<"$0" | cut -d: -f1); [ "$a" -lt "$b" ]' "$log"
check "--no-firewall writes nothing" bash -c 'NECROPSY_INSTALL_SOURCED=1; . "$0"; dry=0; do_firewall=0; LIB_DIR="$1/none"; run() { :; }; install_firewall' "$script" "$fw_dir"

# ---- self-updating ---------------------------------------------------------------

up_dir="$work/up"; mkdir -p "$up_dir/lib" "$up_dir/systemd" "$up_dir/conf"
install_update() { # extra shell run first
  NECROPSY_INSTALL_SOURCED=1 bash -c '
    . "$0"; CONF_DIR="$1/conf"; LIB_DIR="$1/lib"; SYSTEMD_DIR="$1/systemd"; repo_root="$2"
    dry=0; run() { :; }
    listen=10.9.9.9; port=9400; repo=https://github.com/o/r; branch=main; do_update=1
    '"${2:-:}"'
    install_auto_update' "$script" "$up_dir" "$repo_root" 2>/dev/null
}
check "the updater step runs"                     install_update
check "the updater is installed, executable"      test -x "$up_dir/lib/update.sh"
check "...and is the repository's own"            cmp -s "$up_dir/lib/update.sh" "$repo_root/deploy/range/update.sh"
check "its service and timer are installed"       test -f "$up_dir/systemd/necropsy-range-update.service" -a -f "$up_dir/systemd/necropsy-range-update.timer"
check "it updates from the repository named"      grep -qx 'REPO=https://github.com/o/r' "$up_dir/conf/update.env"
check "...from the trusted branch"                grep -qx 'BRANCH=main' "$up_dir/conf/update.env"
check "...and checks the spawner where it listens" grep -qx 'HEALTH_URL=http://10.9.9.9:9400/healthz' "$up_dir/conf/update.env"
check "...restarting the spawner's own unit"      grep -qx 'SERVICE=necropsy-spawner' "$up_dir/conf/update.env"
check "the settings file is not writable by others" equals "$(stat -c %a "$up_dir/conf/update.env")" 644
check "the unit runs the installed updater"       grep -qx 'ExecStart=/usr/local/lib/necropsy-range/update.sh' "$up_dir/systemd/necropsy-range-update.service"
check "the timer checks every few minutes"        grep -q 'OnUnitInactiveSec=5min' "$up_dir/systemd/necropsy-range-update.timer"
check "...and after boot"                         grep -q 'OnBootSec=' "$up_dir/systemd/necropsy-range-update.timer"
rm -f "$up_dir/lib/update.sh"
check "--no-auto-update installs nothing"         bash -c 'NECROPSY_INSTALL_SOURCED=1; . "$0"; dry=0; do_update=0; LIB_DIR="$1/lib"; run() { :; }; install_auto_update' "$script" "$up_dir"
check "...and the updater is not there"           test ! -e "$up_dir/lib/update.sh"
# The updater's own paths and the unit's agree with what this installer writes.
check "the installer's lib directory is the one the unit runs from" bash -c 'grep -q "^LIB_DIR=/usr/local/lib/necropsy-range" "$0"' "$script"
check "the updater's default state directory is the spawner's family" grep -q 'STATE_DIR=/var/lib/necropsy-range/updater' "$repo_root/deploy/range/update.sh"

# ---- the unit -----------------------------------------------------------------------

unit="$repo_root/deploy/range/necropsy-spawner.service"
check "the spawner unit reads where to listen from its settings" grep -qx 'EnvironmentFile=/etc/necropsy-range/spawner.env' "$unit"
check "...and its tenants"                        grep -qx 'Environment=SPAWNER_TENANTS=/etc/necropsy-range/tenants.toml' "$unit"
check "it may only use IPv4 and unix sockets"     grep -qx 'RestrictAddressFamilies=AF_INET AF_UNIX' "$unit"
check "it cannot gain privileges"                 grep -qx 'NoNewPrivileges=yes' "$unit"
check "its lab sockets survive a restart"         grep -qx 'RuntimeDirectoryPreserve=yes' "$unit"
check "the allowlists live in its state directory" grep -qx 'StateDirectory=necropsy-range' "$unit"

# ---- a full dry run -------------------------------------------------------------

out="$(dry "${ok_args[@]}" 2>&1)"
check "a dry run succeeds"                        equals "$?" 0
check "it builds the spawner in a container"      grep -q 'docker run --rm .*-alpine' <<<"$out"
check "it writes both tenants"                    bash -c 'grep -q "tenant prod (10.0.0.21)" <<<"$0" && grep -q "tenant dev (10.0.0.22)" <<<"$0"' "$out"
check "it restricts the port"                     grep -q 'restricting port 9400 to the site VMs' <<<"$out"
check "it starts the spawner"                     grep -q 'systemctl restart necropsy-spawner' <<<"$out"
check "it sets up self-updating from main"        grep -q 'setting up self-updating from main' <<<"$out"
check "...and starts the timer"                   grep -q 'systemctl enable --now necropsy-range-update.timer' <<<"$out"
check "it says the range updates itself"          grep -q 'The range updates itself' <<<"$out"
check "it tells where each token is"              grep -q 'sudo cat /etc/necropsy-range/tokens/prod' <<<"$out"
check "it tells what to run on each site"         bash -c 'grep -q -- "--range-url http://10.9.9.9:9400 --range-tenant prod" <<<"$0" && grep -q -- "--range-tenant dev" <<<"$0"' "$out"
check "it never prints a token"                   fails grep -Eq '[0-9a-f]{64}' <<<"$out"
check "it says nothing was changed"               grep -q 'Dry run: nothing was changed' <<<"$out"
out="$(dry --listen 10.9.9.9 --tenant prod:10.0.0.21 --no-firewall --port 9555 2>&1)"
check "--no-auto-update warns that updating is manual" bash -c 'dry() { bash "$0" --dry-run "$@"; }; dry --listen 10.9.9.9 --tenant prod:10.0.0.22 --no-auto-update 2>&1 | grep -q "no self-updating"' "$script"
check "--no-firewall warns instead"               grep -q 'no firewall' <<<"$out"
check "--port is used"                            grep -q 'http://10.9.9.9:9555' <<<"$out"
check "a dry run changed nothing here" \
  bash -c '[ ! -e /usr/local/bin/necropsy-spawner ] && [ ! -e /etc/necropsy-range ] && [ ! -e /etc/systemd/system/necropsy-spawner.service ]'

echo
if [ "$failures" -eq 0 ]; then echo "all passed"; else echo "$failures failed"; exit 1; fi
