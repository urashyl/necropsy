#!/usr/bin/env bash
# Installs a necropsy site on a VM of its own: the site (blue-green, in Docker),
# its settings, a firewall rule for its port, the connection to the range VM that
# runs live-puzzle labs, and CI deploys. Run it on each site's VM, once for
# production and once for dev, as root, from a checkout of the repository:
#
#   # on the production VM
#   sudo deploy/install-site.sh --domain necropsy.urashyl.dev --proxy-ip 192.168.100.5 \
#        --range-url http://192.168.100.50:9400 --range-tenant prod \
#        --range-token-file /root/range-prod.token \
#        --turnstile-site-key 0x4AAA... --turnstile-secret-file /root/ts-secret \
#        --smtp-host smtp.example.com ...
#
#   # on the dev VM: the same, with its own domain, tenant, token and --branch dev
#
# The range VM is set up first with deploy/install-range.sh; it prints what to
# pass here. Without --range-url the site runs, but container puzzles are hidden.
#
# Secrets are never taken as options (they would show up in `ps` and in your
# shell history): pass them in the environment, or by file.
#   NECROPSY_TURNSTILE_SECRET   or  --turnstile-secret-file FILE
#   NECROPSY_SMTP_PASSWORD
#   --range-token-file FILE
#
# It is safe to run again: it updates what changed, keeps your settings, and
# redeploys. Use --dry-run to see everything it would do first.
#
# What it does:
#   1. installs what is missing (Docker from Docker's own apt repository on
#      Debian or Ubuntu; git, curl, iptables and friends)
#   2. creates an unprivileged deploy user, and installs the blue-green deploy
#      program (deploy/bluegreen/install-system.sh): CI runs `sudo necropsy-deploy`
#      as that user, and nothing else
#   3. writes the settings, and checks the range VM answers and accepts the token
#   4. restricts the site's port to your reverse proxy
#   5. deploys the newest commit of the branch. A deploy builds the site and the
#      lab images, pushes the images the range does not have, and has the range
#      test every lab before any traffic moves
#
# Options:
#   --domain NAME          the site's public name, e.g. necropsy.urashyl.dev
#                          (required). Live puzzles are served at *.NAME
#   --proxy-ip ADDR        the address your reverse proxy connects from. Only it
#                          may reach the site's port, and only it is believed
#                          about visitors' addresses (required, unless
#                          --proxy-on-host)
#   --proxy-on-host        the reverse proxy runs on this VM, on loopback
#   --repo URL             https URL to build releases from (default: this
#                          checkout's origin). It must be reachable without a login
#   --branch NAME          the only branch that is built (main; dev on the dev VM)
#   --deploy-user NAME     the unprivileged account that may run deploys through
#                          sudo, for CI (deploy). Created if it does not exist
#   --range-url URL        the range VM's spawner, like http://192.168.100.50:9400
#   --range-tenant NAME    this site's name at the range: prod or dev
#   --range-token-file FILE  the file holding this site's token (from install-range.sh)
#   --turnstile-site-key K Cloudflare Turnstile site key (public)
#   --turnstile-secret-file FILE
#                          file holding the Turnstile secret key
#   --smtp-host H  --smtp-port P  --smtp-user U  --smtp-from "name <addr>"
#                          outgoing email (accounts need it). TLS is STARTTLS on
#                          587 unless you edit /etc/necropsy/necropsy.env
#   --no-deploy            do everything except deploying the site
#   --no-selftest          do not have the range test the labs on deploy
#   --no-firewall          do not touch the firewall
#   --dry-run              print what would happen and change nothing
#   -h, --help
#
# Files it creates:
#   /etc/necropsy/necropsy.env   the site's settings (mode 600)
#   /etc/necropsy/deploy.env     deploy options (repo, branch, range)
#   /etc/necropsy/range.token    this site's token for the range (mode 600)
#   /usr/local/bin/necropsy-deploy
#   /etc/systemd/system/necropsy-firewall.service

set -euo pipefail
shopt -s inherit_errexit

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$here/.." && pwd)"
# shellcheck source=lib/common.sh
. "$here/lib/common.sh"

usage() { sed -n '2,200p' "${BASH_SOURCE[0]}" | sed -n '1,/^set -euo/p' | sed '$d' | sed 's/^# \{0,1\}//'; }

# ---- settings ----------------------------------------------------------------

domain="" proxy_ip="" proxy_on_host=0 repo="" branch=main deploy_user=deploy
range_url="" range_tenant="" range_token_file=""
site_key="" secret_file="" smtp_host="" smtp_port="" smtp_user="" smtp_from=""
do_deploy=1 do_selftest=1 do_firewall=1 dry=0

CONF_DIR=/etc/necropsy
BIN_DIR=/usr/local/bin
LIB_DIR=/usr/local/lib/necropsy
SYSTEMD_DIR=/etc/systemd/system

# ---- arguments -----------------------------------------------------------------

parse_args() {
  while [ $# -gt 0 ]; do
    case "$1" in
      --domain) domain="${2:?--domain needs a name}"; shift 2 ;;
      --proxy-ip) proxy_ip="${2:?--proxy-ip needs an address}"; shift 2 ;;
      --proxy-on-host) proxy_on_host=1; shift ;;
      --repo) repo="${2:?--repo needs a URL}"; shift 2 ;;
      --branch) branch="${2:?--branch needs a name}"; shift 2 ;;
      --deploy-user) deploy_user="${2:?--deploy-user needs a name}"; shift 2 ;;
      --range-url) range_url="${2:?--range-url needs a URL}"; shift 2 ;;
      --range-tenant) range_tenant="${2:?--range-tenant needs a name}"; shift 2 ;;
      --range-token-file) range_token_file="${2:?--range-token-file needs a file}"; shift 2 ;;
      --turnstile-site-key) site_key="${2:?}"; shift 2 ;;
      --turnstile-secret-file) secret_file="${2:?}"; shift 2 ;;
      --smtp-host) smtp_host="${2:?}"; shift 2 ;;
      --smtp-port) smtp_port="${2:?}"; shift 2 ;;
      --smtp-user) smtp_user="${2:?}"; shift 2 ;;
      --smtp-from) smtp_from="${2:?}"; shift 2 ;;
      --no-deploy) do_deploy=0; shift ;;
      --no-selftest) do_selftest=0; shift ;;
      --no-firewall) do_firewall=0; shift ;;
      --dry-run) dry=1; shift ;;
      -h|--help) usage; exit 0 ;;
      *) usage >&2; die "unknown option: $1" ;;
    esac
  done
}

validate_args() {
  [ -n "$domain" ] || die "--domain is required"
  valid_domain "$domain" || die "--domain must be a plain lower-case domain name like necropsy.urashyl.dev"
  if [ "$proxy_on_host" = 1 ]; then
    [ -z "$proxy_ip" ] || die "use either --proxy-ip or --proxy-on-host, not both"
  else
    [ -n "$proxy_ip" ] || die "--proxy-ip is required (your reverse proxy's address), or --proxy-on-host"
    valid_addr "$proxy_ip" || die "--proxy-ip must be a single IP address, like 192.168.100.5"
  fi
  valid_branch "$branch" || die "--branch must be a plain branch name"
  valid_user "$deploy_user" || die "--deploy-user must be a valid user name, and not root"
  if [ -n "$range_url" ] || [ -n "$range_tenant" ] || [ -n "$range_token_file" ]; then
    [ -n "$range_url" ] && [ -n "$range_tenant" ] && [ -n "$range_token_file" ] \
      || die "--range-url, --range-tenant and --range-token-file go together"
    [[ "$range_url" =~ ^https?://[A-Za-z0-9.-]+(:[0-9]{1,5})?$ ]] || die "--range-url must look like http://192.168.100.50:9400"
    valid_tenant "$range_tenant" || die "--range-tenant must be 1 to 16 lower-case letters and digits, like prod"
    [ -f "$range_token_file" ] || die "$range_token_file does not exist"
    # The whole file, less its final newline, must be the token: spaces or more
    # than one line mean it is the wrong file.
    [[ "$(cat "$range_token_file")" =~ ^[A-Za-z0-9_-]{16,}$ ]] || die "$range_token_file does not look like a token (16 or more letters, digits, - and _, on one line)"
  fi
  [ -z "$smtp_port" ] || [[ "$smtp_port" =~ ^[0-9]+$ ]] || die "--smtp-port must be a number"
  [ -z "$secret_file" ] || [ -f "$secret_file" ] || die "$secret_file does not exist"
  if [ -n "$site_key" ] && [ -z "${NECROPSY_TURNSTILE_SECRET:-}" ] && [ -z "$secret_file" ]; then
    die "a Turnstile site key needs its secret: set NECROPSY_TURNSTILE_SECRET or pass --turnstile-secret-file"
  fi
  if [ -z "$site_key" ] && { [ -n "${NECROPSY_TURNSTILE_SECRET:-}" ] || [ -n "$secret_file" ]; }; then
    die "a Turnstile secret needs its site key (--turnstile-site-key)"
  fi
  if [ -n "$repo" ]; then
    [[ "$repo" =~ ^https://[A-Za-z0-9._/-]+$ ]] || die "--repo must be an https URL like https://github.com/owner/name"
  fi
}

# ---- steps ----------------------------------------------------------------------

preflight() {
  if [ "$dry" = 0 ]; then
    [ "$(id -u)" -eq 0 ] || die "run this as root (sudo)"
    [ "$(uname -s)" = Linux ] || die "this installs onto Linux"
  fi
  [ -f "$repo_root/deploy/bluegreen/install-system.sh" ] \
    || die "run this from a checkout of the necropsy repository (deploy/install-site.sh)"
  repo="$(checkout_repo_url "$repo")"
  info "https://$domain (live puzzles at *.$domain), building $repo ($branch)${range_url:+, labs on the range at $range_url as $range_tenant}"
  [ -n "$range_url" ] || warn "no --range-url: container puzzles stay hidden until the range is set up"
}

install_site_packages() {
  install_packages git:git curl:curl flock:util-linux ip:iproute2 iptables:iptables awk:gawk sudo:sudo visudo:sudo
}

ensure_deploy_user() {
  if id -u "$deploy_user" >/dev/null 2>&1; then
    info "deploy user $deploy_user exists"
    return
  fi
  info "creating the deploy user $deploy_user (no password; add your CI's public key to ~$deploy_user/.ssh/authorized_keys)"
  run useradd --create-home --shell /bin/bash "$deploy_user"
}

install_deploy_program() {
  info "installing the blue-green deploy program"
  run "$repo_root/deploy/bluegreen/install-system.sh" --user "$deploy_user" --repo "$repo" --branch "$branch"
}

# The range VM answers, and accepts this site's token.
check_range() {
  [ -n "$range_url" ] || return 0
  [ "$dry" = 0 ] || { echo "+ check that $range_url answers and accepts the token" >&2; return 0; }
  local token code
  token="$(tr -d '[:space:]' < "$range_token_file")"
  curl -fsS --max-time 5 "$range_url/healthz" >/dev/null 2>&1 || {
    [ "$do_deploy" = 0 ] && { warn "the range at $range_url does not answer yet"; return 0; }
    die "the range at $range_url does not answer. Is install-range.sh done there, and does its firewall let this VM in?"
  }
  code="$(curl -sS --max-time 5 -o /dev/null -w '%{http_code}' -H @<(printf 'X-Necropsy-Token: %s\n' "$token") "$range_url/v1/instances" 2>/dev/null || true)"
  [ "$code" = 200 ] || die "the range rejected this token (HTTP $code). Use the token install-range.sh made for the $range_tenant tenant."
  info "the range answers and accepts the token"
}

write_settings() {
  info "writing the settings"
  if [ "$dry" = 1 ]; then
    echo "+ set in $CONF_DIR/necropsy.env: NECROPSY_BASE_URL NECROPSY_LIVE_DOMAIN NECROPSY_RANGE_URL NECROPSY_RANGE_TOKEN (and captcha and email if given)" >&2
    echo "+ set in $CONF_DIR/deploy.env: NECROPSY_BIND NECROPSY_EDGE_TRUSTED RANGE_URL RANGE_TENANT RANGE_SELFTEST" >&2
    [ -z "$range_url" ] || echo "+ write $CONF_DIR/range.token (mode 600)" >&2
    return
  fi
  local env="$CONF_DIR/necropsy.env" denv="$CONF_DIR/deploy.env"
  [ -f "$env" ] || die "$env was not created by the deploy installer"
  set_env "$env" NECROPSY_BASE_URL "https://$domain"
  set_env "$env" NECROPSY_LIVE_DOMAIN "$domain"

  if [ -n "$range_url" ]; then
    local token
    token="$(tr -d '[:space:]' < "$range_token_file")"
    printf '%s\n' "$token" > "$CONF_DIR/range.token.new"
    chmod 600 "$CONF_DIR/range.token.new"
    mv -f "$CONF_DIR/range.token.new" "$CONF_DIR/range.token"
    set_env "$env" NECROPSY_RANGE_URL "$range_url"
    set_env "$env" NECROPSY_RANGE_TOKEN "$token"
    set_env "$denv" RANGE_URL "$range_url"
    set_env "$denv" RANGE_TENANT "$range_tenant"
    set_env "$denv" RANGE_SELFTEST "$do_selftest"
  fi

  if [ -n "$site_key" ]; then
    local secret="${NECROPSY_TURNSTILE_SECRET:-}"
    [ -z "$secret_file" ] || secret="$(tr -d '[:space:]' < "$secret_file")"
    set_env "$env" NECROPSY_TURNSTILE_SITE_KEY "$site_key"
    set_env "$env" NECROPSY_TURNSTILE_SECRET "$secret"
  else
    warn "no Turnstile keys: anyone (or any script) can create accounts and start labs. Add them to $env and redeploy"
  fi

  if [ -n "$smtp_host" ]; then
    set_env "$env" NECROPSY_SMTP_HOST "$smtp_host"
    set_env "$env" NECROPSY_SMTP_PORT "${smtp_port:-587}"
    [ -z "$smtp_user" ] || set_env "$env" NECROPSY_SMTP_USERNAME "$smtp_user"
    [ -z "${NECROPSY_SMTP_PASSWORD:-}" ] || set_env "$env" NECROPSY_SMTP_PASSWORD "$NECROPSY_SMTP_PASSWORD"
    [ -z "$smtp_from" ] || set_env "$env" NECROPSY_SMTP_FROM "$smtp_from"
  else
    warn "no email settings: registration and password reset stay closed until NECROPSY_SMTP_* are set in $env"
  fi
  chmod 600 "$env"

  # The reverse proxy is another machine (or this one, on loopback).
  if [ "$proxy_on_host" = 1 ]; then
    set_env "$denv" NECROPSY_BIND 127.0.0.1
  else
    set_env "$denv" NECROPSY_BIND 0.0.0.0
    set_env "$denv" NECROPSY_EDGE_TRUSTED "$proxy_ip"
  fi
  chmod 644 "$denv"
}

# Docker's published ports bypass ufw and firewalld, so the site's port is closed
# to everything but your proxy in Docker's own DOCKER-USER chain, by a small unit
# that runs after Docker starts. In that chain the port is the container's own,
# always 8080, whatever it is published as.
install_firewall() {
  if [ "$do_firewall" = 0 ] || [ "$proxy_on_host" = 1 ]; then
    return
  fi
  if [[ "$proxy_ip" == *:* ]]; then
    warn "an IPv6 proxy address: not adding a firewall rule. Restrict the site's port to it yourself"
    return
  fi
  info "restricting the site's port to $proxy_ip"
  run install -d -m 755 "$LIB_DIR"
  write_file "$LIB_DIR/firewall.sh" 755 <<'FIREWALL'
#!/bin/sh
# Only the reverse proxy may reach the site's published port. In the DOCKER-USER
# chain the port is the container's own, always 8080, whatever it is published
# as. Scoped to the external interface so the containers still talk to each other.
set -eu
proxy="$1"
iface="$(ip -o route get 1.1.1.1 2>/dev/null | sed -n 's/.* dev \([^ ]*\).*/\1/p')"
[ -n "$iface" ] || { echo "no default route: cannot work out the external interface" >&2; exit 1; }
while iptables -D DOCKER-USER -i "$iface" -p tcp --dport 8080 ! -s "$proxy" -j DROP 2>/dev/null; do :; done
iptables -I DOCKER-USER -i "$iface" -p tcp --dport 8080 ! -s "$proxy" -j DROP
FIREWALL
  write_file "$SYSTEMD_DIR/necropsy-firewall.service" 644 <<UNIT
[Unit]
Description=necropsy: only the reverse proxy may reach the site
After=docker.service
Requires=docker.service
PartOf=docker.service

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=$LIB_DIR/firewall.sh $proxy_ip

[Install]
WantedBy=multi-user.target docker.service
UNIT
  run systemctl daemon-reload
  run systemctl enable necropsy-firewall
  run systemctl restart necropsy-firewall
}

deploy_site() {
  [ "$do_deploy" = 1 ] || { info "skipping the deploy (--no-deploy). Later: sudo necropsy-deploy deploy <commit on $branch>"; return 0; }
  info "deploying the newest commit of $branch"
  local sha
  if [ "$dry" = 1 ]; then
    echo "+ necropsy-deploy deploy <newest commit of $branch at $repo>" >&2
    return 0
  fi
  sha="$(git ls-remote "$repo" "refs/heads/$branch" | awk '{print $1}' | head -n 1)"
  [[ "$sha" =~ ^[0-9a-f]{40}$ ]] \
    || die "branch $branch was not found at $repo. Push it there first, or pick another with --branch"
  "$BIN_DIR/necropsy-deploy" deploy "$sha"
}

summary() {
  echo >&2
  if [ "$dry" = 1 ]; then
    echo "Dry run: nothing was changed. Run again without --dry-run to install." >&2
  else
    echo "Installed." >&2
  fi
  {
    echo
    echo "Remaining steps on your reverse proxy and in Cloudflare:"
    echo
    echo "  1. DNS: point $domain and *.$domain at your reverse proxy."
    echo "  2. Certificate: *.$domain needs its own wildcard certificate. A certificate"
    echo "     for *.${domain#*.} does not cover it."
    echo "  3. Proxy: send $domain and *.$domain to this VM, with Host passed through and"
    echo "     X-Real-IP set to the visitor's address:"
    echo
    echo "       @necropsy host $domain *.$domain"
    echo "       handle @necropsy {"
    echo "           request_body {"
    echo "               max_size 1MB"
    echo "           }"
    echo "           reverse_proxy http://$(hostname -I 2>/dev/null | awk '{print $1}'):8080 {"
    echo "               flush_interval -1"
    echo "               header_up X-Real-IP {remote_host}"
    echo "               transport http {"
    echo "                   read_timeout 86400s"
    echo "                   write_timeout 86400s"
    echo "               }"
    echo "           }"
    echo "       }"
    echo
    echo "  4. Turnstile: in the Cloudflare dashboard, the widget's allowed hostnames must"
    echo "     include $domain."
    echo "  5. CI: set the secrets in docs/ci-cd.md on the GitHub environment for $branch"
    echo "     (DEPLOY_HOST is this VM; DEPLOY_USER is $deploy_user). Add the key's public half to"
    echo "     $deploy_user's ~/.ssh/authorized_keys here."
    echo
    if [ "$do_deploy" = 1 ] && [ "$dry" = 0 ]; then
      echo "The first deploy created the admin account. The credentials are shown once:"
      echo
      echo "  docker logs necropsy-blue 2>&1 | grep -A4 'first run'     (necropsy-green if blue is idle)"
      echo
    fi
    echo "Day to day:"
    echo "  sudo necropsy-deploy status | deploy <commit> | rollback"
    echo "  sudoedit $CONF_DIR/necropsy.env     then deploy to apply"
  } >&2
}

main() {
  parse_args "$@"
  validate_args
  preflight
  install_site_packages
  ensure_deploy_user
  install_deploy_program
  check_range
  write_settings
  install_firewall
  deploy_site
  summary
}

# The tests source this file to call the helpers, and set NECROPSY_INSTALL_SOURCED.
if [ -z "${NECROPSY_INSTALL_SOURCED:-}" ]; then
  main "$@"
fi
