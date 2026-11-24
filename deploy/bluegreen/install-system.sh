#!/usr/bin/env bash
# Installs the blue-green deploy program as a root-owned command and lets one
# unprivileged user run it through sudo, without a password and for nothing else.
#
#   sudo install-system.sh --user <name> --repo <https url>
#   sudo install-system.sh --uninstall
#
# After this, <name> can run exactly one program as root, and it accepts only:
#   necropsy-deploy deploy <40-hex commit>
#   necropsy-deploy rollback
#   necropsy-deploy status
# The program, the compose and Caddy files it uses, and its settings are all
# owned by root, so the deploy user cannot change what runs. Changing any of
# them means running this installer again as root.
#
# Options:
#   --user NAME        the account that may run it (required to install). It
#                      must not be root, and must not be in the docker group,
#                      since that would hand it root anyway
#   --repo URL         https URL of the repository to build from (required the
#                      first time; kept in the settings afterwards)
#   --prefix DIR       programs go in DIR/bin and DIR/lib/necropsy   (/usr/local)
#   --sysconfdir DIR   settings go in DIR/necropsy                   (/etc)
#   --statedir DIR     state goes in DIR/necropsy                    (/var/lib)
#   --sudoers-dir DIR  where the sudo rule is written                (/etc/sudoers.d)
#   --allow-docker-group   install even though the user is in the docker group
#   --extra-path DIR   put DIR first in the program's PATH (for tests only)
#   --uninstall        remove the program and sudo rule, keeping settings and state
#   -h, --help

set -euo pipefail

die() { echo "error: $*" >&2; exit 1; }
info() { echo "==> $*"; }
usage() { sed -n '2,27p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }

user="" repo="" prefix=/usr/local sysconfdir=/etc statedir=/var/lib sudoers_dir=/etc/sudoers.d
allow_docker_group=0 uninstall=0 safe_mode=1 extra_path=""

while [ $# -gt 0 ]; do
  case "$1" in
    --user) user="${2:?--user needs a name}"; shift 2 ;;
    --repo) repo="${2:?--repo needs a URL}"; shift 2 ;;
    --prefix) prefix="${2:?}"; shift 2 ;;
    --sysconfdir) sysconfdir="${2:?}"; shift 2 ;;
    --statedir) statedir="${2:?}"; shift 2 ;;
    --sudoers-dir) sudoers_dir="${2:?}"; shift 2 ;;
    --allow-docker-group) allow_docker_group=1; shift ;;
    --extra-path) extra_path="${2:?}"; shift 2 ;;
    --uninstall) uninstall=1; shift ;;
    # For the test suite: an install that does not require root, resets nothing,
    # and accepts file:// repositories. Never use this for a real install.
    --unsafe-for-tests) safe_mode=0; shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; die "unknown option: $1" ;;
  esac
done

if [ "$safe_mode" = 1 ]; then
  [ "$(id -u)" -eq 0 ] || die "run this as root"
fi

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$here/../.." && pwd)"

bin_dir="$prefix/bin"
lib_dir="$prefix/lib/necropsy"
conf_dir="$sysconfdir/necropsy"
state_dir="$statedir/necropsy"
program="$bin_dir/necropsy-deploy"
sudoers_file="$sudoers_dir/necropsy-deploy"

# These are written into a shell script and a sudoers file.
for p in "$program" "$lib_dir" "$conf_dir" "$state_dir" "$sudoers_file"; do
  [[ "$p" =~ ^/[A-Za-z0-9._/+-]+$ ]] || die "unsafe path: $p (use absolute paths with letters, digits and . _ / + -)"
done

if [ "$uninstall" -eq 1 ]; then
  rm -f "$program" "$sudoers_file"
  rm -rf "$lib_dir"
  info "removed $program, $lib_dir and $sudoers_file"
  echo "kept: $conf_dir (settings) and $state_dir (state). The Docker containers keep running;"
  echo "stop them with: docker rm -f necropsy-blue necropsy-green necropsy-proxy"
  exit 0
fi

# ---- checks -----------------------------------------------------------------

[ -n "$user" ] || die "--user is required"
[[ "$user" =~ ^[a-z_][a-z0-9_-]*$ ]] || die "not a valid user name: $user"
id -u "$user" >/dev/null 2>&1 || die "no such user: $user"
[ "$(id -u "$user")" -ne 0 ] || die "the deploy user must not be root"

if id -nG "$user" | tr ' ' '\n' | grep -qx docker; then
  if [ "$allow_docker_group" -eq 1 ]; then
    echo "warning: $user is in the docker group, which is already root-equivalent." >&2
  else
    die "$user is in the docker group, which is root-equivalent, so the sudo rule would add nothing. Remove it (gpasswd -d $user docker) or pass --allow-docker-group."
  fi
fi

for tool in docker git flock curl; do
  command -v "$tool" >/dev/null || die "$tool is required but not installed"
done
docker compose version >/dev/null 2>&1 || die "the docker compose plugin is required"
if [ "$safe_mode" = 1 ]; then
  command -v visudo >/dev/null || die "visudo not found (install sudo)"
fi

existing_repo=""
if [ -f "$conf_dir/deploy.env" ]; then
  existing_repo="$({ grep '^NECROPSY_REPO=' "$conf_dir/deploy.env" || true; } | tail -n 1 | cut -d= -f2-)"
fi
repo="${repo:-$existing_repo}"
[ -n "$repo" ] || die "--repo is required the first time"
if [ "$safe_mode" = 1 ]; then
  [[ "$repo" =~ ^https://[A-Za-z0-9._/-]+$ ]] || die "--repo must be an https URL like https://github.com/owner/name"
else
  [[ "$repo" =~ ^(https|file)://[A-Za-z0-9._/:-]+$ ]] || die "--repo must be an https URL"
fi

# ---- install ----------------------------------------------------------------

info "installing"
install -d -m 755 "$bin_dir" "$lib_dir" "$lib_dir/bluegreen"
install -d -m 750 "$conf_dir"
install -d -m 700 "$state_dir"

install -m 644 "$here/compose.yaml" "$lib_dir/bluegreen/compose.yaml"
install -m 644 "$here/Caddyfile" "$lib_dir/bluegreen/Caddyfile"
install -m 644 "$repo_root/.env.example" "$lib_dir/env.example"

safe_path="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
if [ -n "$extra_path" ]; then
  [[ "$extra_path" =~ ^/[A-Za-z0-9._/+-]+$ ]] || die "unsafe --extra-path"
  safe_path="$extra_path:$safe_path"
fi

# Write the program under a temporary name and rename it, so a running deploy is
# never overwritten half way.
sed \
  -e "s|@LIBDIR@|$lib_dir|g" \
  -e "s|@SYSCONFDIR@|$sysconfdir|g" \
  -e "s|@STATEDIR@|$statedir|g" \
  -e "s|@SAFE_PATH@|$safe_path|g" \
  -e "s|@SAFE_MODE@|$safe_mode|g" \
  "$here/deploy.sh" > "$program.new"
chmod 755 "$program.new"
mv -f "$program.new" "$program"

# Settings: deploy.env is rewritten only for the repository, keeping anything
# the admin added. necropsy.env is created once and never overwritten.
if [ -f "$conf_dir/deploy.env" ] && grep -q '^NECROPSY_REPO=' "$conf_dir/deploy.env"; then
  sed "s|^NECROPSY_REPO=.*|NECROPSY_REPO=$repo|" "$conf_dir/deploy.env" > "$conf_dir/deploy.env.new"
else
  { echo "NECROPSY_REPO=$repo"; [ ! -f "$conf_dir/deploy.env" ] || cat "$conf_dir/deploy.env"; } > "$conf_dir/deploy.env.new"
fi
chmod 644 "$conf_dir/deploy.env.new"
mv -f "$conf_dir/deploy.env.new" "$conf_dir/deploy.env"
[ -f "$conf_dir/deploy.env.example" ] || install -m 644 "$here/deploy.env.example" "$conf_dir/deploy.env.example"

if [ ! -f "$conf_dir/necropsy.env" ]; then
  install -m 600 "$repo_root/.env.example" "$conf_dir/necropsy.env"
  info "created $conf_dir/necropsy.env (edit it, then deploy to apply)"
fi

# ---- sudo -------------------------------------------------------------------

# The rule names the program and nothing else. It deliberately has no argument
# patterns: sudo's pattern syntax differs between sudo and sudo-rs (which
# rejects wildcards in arguments), and the program is the boundary anyway. It
# takes only deploy <40-hex commit>, rollback and status, and refuses everything
# else, including the test-only `switch`.
rule="$(mktemp)"
trap 'rm -f "$rule"' EXIT
cat > "$rule" <<RULE
# Managed by necropsy install-system.sh. Do not edit: rerun the installer.
$user ALL=(root) NOPASSWD: $program
RULE
if [ "$safe_mode" = 1 ]; then
  # A broken sudoers file can lock everyone out of sudo, so check before installing.
  visudo -cf "$rule" >/dev/null || die "the generated sudo rule did not validate; nothing was installed"
fi
install -d -m 750 "$sudoers_dir"
install -m 440 "$rule" "$sudoers_file"

cat <<MSG

Installed.
  program    $program
  settings   $conf_dir/necropsy.env  (the app)    $conf_dir/deploy.env  (deploy options)
  state      $state_dir
  sudo rule  $sudoers_file

$user may now run, as root and without a password, this one program:
  sudo necropsy-deploy deploy <40-hex commit on main>
  sudo necropsy-deploy rollback
  sudo necropsy-deploy status
It accepts nothing else, and the user has no other passwordless sudo.
MSG
