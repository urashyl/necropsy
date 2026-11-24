# Running as a service

`deploy/install.sh` installs necropsy as a systemd **user** service, so it runs as your own user with no root and no system-wide files.

```bash
deploy/install.sh
```

It builds a release binary, installs everything, enables the service and starts it. Run it again to update: it replaces the binary and packs, keeps your settings and database, and restarts the service. It needs `cargo` and a systemd user session, or a prebuilt binary via `NECROPSY_BIN=/path/to/necropsy-web`.

## Where things go

Nothing is hardcoded. Paths come from the XDG variables, falling back to their defaults under `$HOME`, and the checkout is found from the script's own location.

| What | Location |
| --- | --- |
| Binary | `$XDG_BIN_HOME` or `~/.local/bin/necropsy-web` |
| Packs | `$XDG_DATA_HOME` or `~/.local/share`, then `necropsy/packs` |
| Database | `$XDG_STATE_HOME` or `~/.local/state`, then `necropsy/necropsy.db` |
| Settings | `$XDG_CONFIG_HOME` or `~/.config`, then `necropsy/necropsy.env` (mode 0600, never overwritten) |
| Unit | `$XDG_CONFIG_HOME` or `~/.config`, then `systemd/user/necropsy.service` |

The unit is generated from `deploy/necropsy.service.in`, which uses placeholders instead of paths. Listen address, base URL, SMTP and the rest are set in the settings file, documented in `deploy/necropsy.env.example`. After editing it:

```bash
systemctl --user restart necropsy
```

## Running without a login session

A user service normally stops when you log out. The installer runs `loginctl enable-linger` so it starts at boot and keeps running. If that is not allowed for your user, it warns and the service only runs while you are logged in. Skip lingering with `--no-linger`.

## First-run admin credentials

The first start generates an admin account and prints its credentials once, to the journal:

```bash
journalctl --user -u necropsy.service | grep -A4 'first run'
```

## Common tasks

```bash
systemctl --user status necropsy
journalctl --user -u necropsy.service -f
deploy/install.sh --uninstall
```

Uninstalling removes the service and binary but keeps the settings, packs and database. The installer prints their locations.

## Options

| Option | |
| --- | --- |
| `--no-linger` | Do not enable lingering. |
| `--no-start` | Install and enable, but do not start or restart. |
| `--files-only` | Copy files only and never call `systemctl`. Useful for packaging or testing. |
| `--uninstall` | Remove the service and binary. |

## Behind a reverse proxy

Keep `NECROPSY_ADDR` on loopback, point the proxy at it, and set `NECROPSY_BASE_URL` to the public https address. If the proxy appends `X-Forwarded-For` (nginx `proxy_add_x_forwarded_for`), set `NECROPSY_TRUST_PROXY=1` so rate limits see real client addresses. The proxy must pass the `Host` header.
