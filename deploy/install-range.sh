#!/usr/bin/env bash
# Installs the range on a VM of its own: the spawner that runs live-puzzle labs for
# the sites, Docker, and a firewall that lets only the site VMs reach it. Run it
# once, on the container VM, as root, from a checkout of the repository:
#
#   sudo deploy/install-range.sh --listen 192.168.100.50 \
#        --tenant prod:192.168.100.21 --tenant dev:192.168.100.22
#
# A tenant is a site: its name, and the address of its VM. Each gets its own token
# and its own share of the VM, and cannot see, stop or starve another's labs. The
# first tenant listed gets twice a share of the others unless you say otherwise
# with NAME:IP:SHARE (a relative weight).
#
# Afterwards it tells you where each tenant's token is, and what to run on each
# site's VM (deploy/install-site.sh --range-url ... --range-token-file ...).
#
# It is safe to run again: it updates the spawner program and the limits and keeps
# the tokens, so the sites keep working. Use --dry-run to see everything first.
#
# What it does:
#   1. installs what is missing: Docker from Docker's own apt repository on Debian
#      or Ubuntu, and iptables, curl and friends
#   2. builds the spawner program inside a throwaway Rust container, so this VM
#      needs no Rust, and as a static binary, so it runs on any distribution
#   3. writes the tenants (a token each, kept only as a hash by the spawner) with
#      limits sized to this VM's memory
#   4. installs the spawner as a systemd service, listening on the private network
#   5. restricts its port to the site VMs (and this VM itself)
#
# Options:
#   --listen IP            this VM's address on the private network (required).
#                          The spawner is not reachable from anywhere else
#   --port N               the port (9400)
#   --tenant NAME:IP[:SHARE]  a site that may use this range (required, repeat)
#   --reserve-mb N         memory kept for the OS, Docker and the spawner (1024)
#   --no-firewall          do not touch the firewall
#   --dry-run              print what would happen and change nothing
#   -h, --help
#
# Files it creates:
#   /usr/local/bin/necropsy-spawner         the spawner program (static binary)
#   /etc/necropsy-range/tenants.toml        who may use it, and their limits
#   /etc/necropsy-range/spawner.env         where it listens
#   /etc/necropsy-range/tokens/NAME         each tenant's token (root only)
#   /etc/systemd/system/necropsy-spawner.service
#   /etc/systemd/system/necropsy-range-firewall.service
#   /var/lib/necropsy-range/                allowlists and uploads (the spawner's)
#   /run/necropsy-range/                    each lab's socket directory (tmpfs)

set -euo pipefail
shopt -s inherit_errexit

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$here/.." && pwd)"
# shellcheck source=lib/common.sh
. "$here/lib/common.sh"

usage() { sed -n '2,200p' "${BASH_SOURCE[0]}" | sed -n '1,/^set -euo/p' | sed '$d' | sed 's/^# \{0,1\}//'; }

# ---- settings ----------------------------------------------------------------

listen="" port=9400 reserve_mb=1024 do_firewall=1 dry=0
tenant_args=()

CONF_DIR=/etc/necropsy-range
BIN_DIR=/usr/local/bin
LIB_DIR=/usr/local/lib/necropsy-range
SYSTEMD_DIR=/etc/systemd/system

# ---- pure helpers (tested by deploy/test/install-range.sh) -----------------------

# Planning figures, as in docs/range.md: a lab is limited to 128 MB, and with
# Docker's overhead costs 143 MB; a little over what fits is allowed, since few
# labs reach their limit.
LAB_LIMIT_MB=128
LAB_COST_MB=143

# A tenant's limits from its share of the VM: "<max labs> <memory budget MB>".
tenant_limits() { # capacity_mb weight total_weight
  local share=$(( $1 * $2 / $3 ))
  local n=$(( share * 11 / 10 / LAB_COST_MB ))
  [ "$n" -ge 2 ] || n=2
  echo "$n $(( n * LAB_LIMIT_MB ))"
}

# The hex SHA-256 of a token, which is all the spawner keeps of it.
token_hash() { printf '%s' "$1" | sha256sum | cut -d' ' -f1; }

new_token() { head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n'; }

# ---- arguments -----------------------------------------------------------------

parse_args() {
  while [ $# -gt 0 ]; do
    case "$1" in
      --listen) listen="${2:?--listen needs an address}"; shift 2 ;;
      --port) port="${2:?--port needs a number}"; shift 2 ;;
      --tenant) tenant_args+=("${2:?--tenant needs NAME:IP}"); shift 2 ;;
      --reserve-mb) reserve_mb="${2:?--reserve-mb needs a number}"; shift 2 ;;
      --no-firewall) do_firewall=0; shift ;;
      --dry-run) dry=1; shift ;;
      -h|--help) usage; exit 0 ;;
      *) usage >&2; die "unknown option: $1" ;;
    esac
  done
}

# Parsed tenants, in order: names, addresses, weights.
t_names=() t_ips=() t_weights=()

validate_args() {
  [ -n "$listen" ] || die "--listen is required: this VM's address on the private network"
  valid_ipv4 "$listen" || die "--listen must be an IPv4 address, like 192.168.100.50"
  [[ "$port" =~ ^[0-9]+$ ]] && [ "$port" -ge 1024 ] && [ "$port" -le 65535 ] || die "--port must be a number from 1024 to 65535"
  [[ "$reserve_mb" =~ ^[0-9]+$ ]] && [ "$reserve_mb" -ge 256 ] || die "--reserve-mb must be a number, at least 256"
  [ ${#tenant_args[@]} -gt 0 ] || die "at least one --tenant NAME:IP is required"
  local a name ip weight i=0 seen=" "
  for a in "${tenant_args[@]}"; do
    IFS=: read -r name ip weight <<<"$a"
    valid_tenant "$name" || die "tenant name $name must be 1 to 16 lower-case letters and digits, starting with a letter"
    case "$seen" in *" $name "*) die "tenant $name is listed twice" ;; esac
    seen="$seen$name "
    valid_ipv4 "$ip" || die "tenant $name: $ip is not an IPv4 address (use NAME:IP, like prod:192.168.100.21)"
    if [ -z "$weight" ]; then
      if [ "$i" -eq 0 ] && [ ${#tenant_args[@]} -gt 1 ]; then weight=2; else weight=1; fi
    fi
    [[ "$weight" =~ ^[0-9]+$ ]] && [ "$weight" -ge 1 ] && [ "$weight" -le 100 ] || die "tenant $name: the share must be a number from 1 to 100"
    t_names+=("$name") t_ips+=("$ip") t_weights+=("$weight")
    i=$((i + 1))
  done
}

# ---- steps ----------------------------------------------------------------------

preflight() {
  if [ "$dry" = 0 ]; then
    [ "$(id -u)" -eq 0 ] || die "run this as root (sudo)"
    [ "$(uname -s)" = Linux ] || die "this installs onto Linux"
  fi
  [ -f "$repo_root/crates/necropsy-spawner/Cargo.toml" ] \
    || die "run this from a checkout of the necropsy repository (deploy/install-range.sh)"
  total_mb=$(( $(awk '/^MemTotal:/ {print $2}' /proc/meminfo) / 1024 ))
  capacity_mb=$(( total_mb - reserve_mb ))
  info "this VM: $(nproc) vCPU, ${total_mb} MB, of which ${capacity_mb} MB for labs"
  [ "$capacity_mb" -ge 512 ] || die "not enough memory: ${total_mb} MB leaves ${capacity_mb} MB for labs after the ${reserve_mb} MB reserve"
  if [ "$dry" = 0 ] && ! ip -o addr show 2>/dev/null | grep -qw "$listen"; then
    die "$listen is not an address of this VM (see: ip addr)"
  fi
}

install_range_packages() {
  install_packages curl:curl iptables:iptables awk:gawk sha256sum:coreutils od:coreutils
}

# Built in a Rust container, as a static (musl) binary: this VM needs no Rust
# toolchain, and the result does not depend on its libc.
build_spawner() {
  info "building the spawner"
  local rust_version out
  rust_version="$(sed -n 's/^ARG RUST_VERSION=//p' "$repo_root/Dockerfile" | head -n 1)"
  [ -n "$rust_version" ] || die "cannot find the Rust version in the Dockerfile"
  out="$(mktemp -d)"
  run docker run --rm \
    -v "$repo_root":/src:ro \
    -v necropsy-build-cargo:/usr/local/cargo/registry \
    -v necropsy-build-target:/target \
    -v "$out":/out \
    -e CARGO_TARGET_DIR=/target \
    -w /src \
    "rust:${rust_version}-alpine" \
    sh -c 'apk add --no-cache musl-dev >/dev/null && cargo build --release --locked -p necropsy-spawner && cp /target/release/necropsy-spawner /out/'
  run install -d -m 755 "$BIN_DIR"
  if [ "$dry" = 0 ]; then
    [ -x "$out/necropsy-spawner" ] || die "the spawner did not build"
    install -m 755 "$out/necropsy-spawner" "$BIN_DIR/necropsy-spawner.new"
    mv -f "$BIN_DIR/necropsy-spawner.new" "$BIN_DIR/necropsy-spawner"
    rm -rf "$out"
  else
    echo "+ install the built binary as $BIN_DIR/necropsy-spawner" >&2
  fi
}

# The tenants file, and a token for each. A token already made is kept, so the
# sites keep working when this runs again.
write_tenants() {
  info "writing the tenants"
  local sum=0 w i name token text="" limits n budget
  for w in "${t_weights[@]}"; do sum=$((sum + w)); done
  run install -d -m 750 "$CONF_DIR" "$CONF_DIR/tokens"
  for i in "${!t_names[@]}"; do
    name="${t_names[$i]}"
    if [ -s "$CONF_DIR/tokens/$name" ]; then
      token="$(tr -d '[:space:]' < "$CONF_DIR/tokens/$name")"
    else
      token="$(new_token)"
      if [ "$dry" = 0 ]; then
        printf '%s\n' "$token" > "$CONF_DIR/tokens/$name.new"
        chmod 600 "$CONF_DIR/tokens/$name.new"
        mv -f "$CONF_DIR/tokens/$name.new" "$CONF_DIR/tokens/$name"
      else
        echo "+ write $CONF_DIR/tokens/$name (a new token, mode 600)" >&2
      fi
    fi
    limits="$(tenant_limits "$capacity_mb" "${t_weights[$i]}" "$sum")"
    n="${limits% *}" budget="${limits#* }"
    info "tenant $name (${t_ips[$i]}): at most $n labs, $budget MB"
    text+="[tenants.$name]
token_sha256 = \"$(token_hash "$token")\"
max_instances = $n
memory_budget_mb = $budget

"
  done
  printf '%s' "$text" | write_file "$CONF_DIR/tenants.toml" 640
  printf 'SPAWNER_LISTEN=%s:%s\n' "$listen" "$port" | write_file "$CONF_DIR/spawner.env" 644
}

install_spawner_service() {
  info "installing the spawner service"
  if [ "$dry" = 0 ]; then
    install -m 644 "$repo_root/deploy/range/necropsy-spawner.service" "$SYSTEMD_DIR/necropsy-spawner.service"
  else
    echo "+ install necropsy-spawner.service" >&2
  fi
  run systemctl daemon-reload
  run systemctl enable necropsy-spawner
  run systemctl restart necropsy-spawner
  if [ "$dry" = 0 ]; then wait_for_spawner "http://$listen:$port/healthz"; fi
}

wait_for_spawner() { # url
  local i
  for i in $(seq 1 50); do
    curl -fsS --max-time 2 "$1" >/dev/null 2>&1 && return 0
    sleep 0.2
  done
  die "the spawner did not start. See: journalctl -u necropsy-spawner -n 50"
}

# Labs have no network and Docker publishes nothing here, so the one thing this VM
# exposes is the spawner's port. Only the site VMs (and this VM) may reach it.
install_firewall() {
  if [ "$do_firewall" = 0 ]; then
    warn "no firewall: the spawner's port is open to anything that can reach $listen. Tokens are all that protect it"
    return
  fi
  info "restricting port $port to the site VMs"
  run install -d -m 755 "$LIB_DIR"
  write_file "$LIB_DIR/firewall.sh" 755 <<'FIREWALL'
#!/bin/sh
# Only the sites may reach the spawner's port: usage firewall.sh PORT IP...
# A chain of its own, rebuilt each time, so changing the sites changes the rules.
set -eu
port="$1"; shift
iptables -N NECROPSY-RANGE 2>/dev/null || true
iptables -F NECROPSY-RANGE
iptables -A NECROPSY-RANGE -i lo -j ACCEPT
for ip in "$@"; do
  iptables -A NECROPSY-RANGE -s "$ip" -j ACCEPT
done
iptables -A NECROPSY-RANGE -j DROP
# Drop any earlier jump to it (the port may have changed), then add the current one.
iptables -S INPUT | grep -- '-j NECROPSY-RANGE' | sed 's/^-A /-D /' | while read -r rule; do
  # shellcheck disable=SC2086
  iptables $rule
done
iptables -I INPUT -p tcp --dport "$port" -j NECROPSY-RANGE
FIREWALL
  write_file "$SYSTEMD_DIR/necropsy-range-firewall.service" 644 <<UNIT
[Unit]
Description=necropsy range: only the site VMs may reach the spawner
Before=necropsy-spawner.service
After=network-pre.target

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=$LIB_DIR/firewall.sh $port ${t_ips[*]}

[Install]
WantedBy=multi-user.target
UNIT
  run systemctl daemon-reload
  run systemctl enable necropsy-range-firewall
  run systemctl restart necropsy-range-firewall
}

summary() {
  echo >&2
  if [ "$dry" = 1 ]; then
    echo "Dry run: nothing was changed. Run again without --dry-run to install." >&2
  else
    echo "Installed. The range is at http://$listen:$port" >&2
  fi
  local i name
  {
    echo
    echo "Each site has its own token, kept root-only on this VM:"
    echo
    for i in "${!t_names[@]}"; do
      name="${t_names[$i]}"
      echo "  $name (from ${t_ips[$i]}):  sudo cat $CONF_DIR/tokens/$name"
    done
    echo
    echo "Copy a token to its site's VM as a root-only file, then on that VM run:"
    echo
    for i in "${!t_names[@]}"; do
      name="${t_names[$i]}"
      echo "  # the $name site"
      echo "  sudo deploy/install-site.sh --range-url http://$listen:$port --range-tenant $name \\"
      echo "       --range-token-file /root/range-$name.token  (and its --domain, --proxy-ip, ...)"
    done
    echo
    echo "Day to day:"
    echo "  systemctl status necropsy-spawner     journalctl -u necropsy-spawner -f"
    echo "  docker ps --filter label=necropsy.range=1          (the running labs)"
    echo "  sudoedit $CONF_DIR/tenants.toml     then: systemctl restart necropsy-spawner"
    echo "Run this script again to update the spawner program or the limits (tokens are kept)."
  } >&2
}

main() {
  parse_args "$@"
  validate_args
  preflight
  install_range_packages
  build_spawner
  write_tenants
  install_firewall
  install_spawner_service
  summary
}

# The tests source this file to call the helpers, and set NECROPSY_INSTALL_SOURCED.
if [ -z "${NECROPSY_INSTALL_SOURCED:-}" ]; then
  main "$@"
fi
