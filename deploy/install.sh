#!/usr/bin/env bash
# Installs necropsy as a systemd user service. Run it from a checkout:
#
#   deploy/install.sh              build, install and start (or update and restart)
#   deploy/install.sh --uninstall  stop and remove the service and binary
#
# Nothing is hardcoded: every path comes from the XDG variables (or their
# defaults under $HOME), and the checkout is found from this script's own
# location. Re-running it updates the binary and packs, and never touches the
# environment file or the database.
#
# Options:
#   --no-linger   do not run `loginctl enable-linger` (the service then only
#                 runs while you are logged in)
#   --no-start    install and enable, but do not start or restart
#   --files-only  install files only, without calling systemctl at all
#   --uninstall   remove the service and binary, keeping config, data and state
#   -h, --help
#
# Environment overrides:
#   NECROPSY_BIN  use this prebuilt binary instead of running cargo

set -euo pipefail

usage() { sed -n '2,22p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }
die() { echo "error: $*" >&2; exit 1; }
info() { echo "==> $*"; }

linger=1 start=1 systemd=1 uninstall=0
for arg in "$@"; do
  case "$arg" in
    --no-linger) linger=0 ;;
    --no-start) start=0 ;;
    --files-only) systemd=0; linger=0; start=0 ;;
    --uninstall) uninstall=1 ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; die "unknown option: $arg" ;;
  esac
done

[ -n "${HOME:-}" ] || die "HOME is not set"
# A user service belongs to a normal login, not root. --files-only only copies
# files, so it is allowed anywhere (packaging, containers, tests).
[ "$systemd" -eq 0 ] || [ "$(id -u)" -ne 0 ] \
  || die "run this as your own user, not root (it installs a user service)"

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

bin_dir="${XDG_BIN_HOME:-$HOME/.local/bin}"
data_dir="${XDG_DATA_HOME:-$HOME/.local/share}/necropsy"
state_dir="${XDG_STATE_HOME:-$HOME/.local/state}/necropsy"
config_dir="${XDG_CONFIG_HOME:-$HOME/.config}/necropsy"
unit_dir="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"

bin="$bin_dir/necropsy-web"
packs="$data_dir/packs"
db="$state_dir/necropsy.db"
env_file="$config_dir/necropsy.env"
unit="$unit_dir/necropsy.service"

# Unit files treat whitespace, quotes and % specially. Refuse rather than
# write a unit that quietly means something else.
for p in "$bin" "$packs" "$db" "$env_file" "$state_dir"; do
  case "$p" in
    *[[:space:]\"\'\\%]*) die "path contains a space, quote, backslash or %: $p" ;;
  esac
done

ctl() { systemctl --user "$@"; }

need_systemd() {
  command -v systemctl >/dev/null || die "systemctl not found (use --files-only to skip systemd)"
  ctl show-environment >/dev/null 2>&1 \
    || die "cannot reach your systemd user manager (is this a login session? use --files-only to skip systemd)"
}

if [ "$uninstall" -eq 1 ]; then
  if [ "$systemd" -eq 1 ]; then
    need_systemd
    ctl disable --now necropsy.service 2>/dev/null || true
  fi
  rm -f "$unit" "$bin"
  [ "$systemd" -eq 0 ] || ctl daemon-reload
  info "removed the service and binary"
  echo "kept (delete by hand if you want them gone):"
  echo "  config  $config_dir"
  echo "  data    $data_dir"
  echo "  state   $state_dir  (the database, including accounts)"
  exit 0
fi

[ "$systemd" -eq 0 ] || need_systemd
[ -d "$repo/packs" ] || die "no packs directory in $repo (run this from a necropsy checkout)"
[ -f "$repo/deploy/necropsy.service.in" ] || die "missing deploy/necropsy.service.in"

# ---- binary ---------------------------------------------------------------

if [ -n "${NECROPSY_BIN:-}" ]; then
  [ -x "$NECROPSY_BIN" ] || die "NECROPSY_BIN is not an executable file: $NECROPSY_BIN"
  built="$NECROPSY_BIN"
else
  command -v cargo >/dev/null || die "cargo not found (install Rust, or set NECROPSY_BIN to a prebuilt binary)"
  info "building (release)"
  (cd "$repo" && cargo build --release --locked -p necropsy-web)
  built="${CARGO_TARGET_DIR:-$repo/target}/release/necropsy-web"
  [ -x "$built" ] || die "build did not produce $built"
fi

# ---- files ----------------------------------------------------------------

info "installing into $bin_dir, $data_dir, $state_dir, $config_dir"
mkdir -p "$bin_dir" "$data_dir" "$state_dir" "$config_dir" "$unit_dir"

# Copy to a temporary name and rename, since overwriting a running binary
# fails with "text file busy".
install -m 755 "$built" "$bin.new"
mv -f "$bin.new" "$bin"

# Packs are replaced whole so a removed pack does not linger.
rm -rf "$packs.new"
cp -R "$repo/packs" "$packs.new"
rm -rf "$packs"
mv "$packs.new" "$packs"

# The environment file holds settings and possibly the SMTP password: create
# it once, private, and never overwrite it.
if [ ! -e "$env_file" ]; then
  install -m 600 "$repo/.env.example" "$env_file"
  info "created $env_file (edit it, then restart the service)"
else
  chmod 600 "$env_file"
fi

# Fill in the unit template. `|` is the sed delimiter; paths were checked above.
sed \
  -e "s|@BIN@|$bin|g" \
  -e "s|@STATE_DIR@|$state_dir|g" \
  -e "s|@PACKS@|$packs|g" \
  -e "s|@DB@|$db|g" \
  -e "s|@ENV_FILE@|$env_file|g" \
  "$repo/deploy/necropsy.service.in" > "$unit.new"
mv -f "$unit.new" "$unit"

# ---- systemd --------------------------------------------------------------

if [ "$systemd" -eq 1 ]; then
  ctl daemon-reload
  ctl enable necropsy.service

  if [ "$linger" -eq 1 ]; then
    if loginctl enable-linger "$(id -un)" 2>/dev/null; then
      info "lingering enabled: the service starts at boot and keeps running after you log out"
    else
      echo "warning: could not enable lingering. The service will only run while you are logged in." >&2
      echo "         Ask an admin to run: loginctl enable-linger $(id -un)" >&2
    fi
  else
    echo "note: lingering not enabled, so the service only runs while you are logged in." >&2
  fi

  if [ "$start" -eq 1 ]; then
    ctl restart necropsy.service
    sleep 2
    if ctl is-active --quiet necropsy.service; then
      info "necropsy is running"
    else
      echo "error: the service did not stay up. Recent log:" >&2
      journalctl --user -u necropsy.service -n 20 --no-pager >&2 || true
      exit 1
    fi
  fi

  cat <<MSG

Logs:      journalctl --user -u necropsy.service -f
Settings:  $env_file   (then: systemctl --user restart necropsy)
On the first start the server prints generated admin credentials to the log,
once. Show them with:
  journalctl --user -u necropsy.service | grep -A4 'first run'
MSG
else
  cat <<MSG

Files installed. To run it under systemd later, re-run without --files-only.
Unit:      $unit
Settings:  $env_file
MSG
fi
