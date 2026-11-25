# necropsy

Short puzzles built from real findings, such as malware, browser extensions and decrypted profile data. Each puzzle is a pack: a brief, some files, an answer and an explainer. A few intro stages teach the basics, then a new daily puzzle is released each day and the old ones stay playable.

Demo site: <https://necropsy.urashyl.dev>

```bash
cargo run -p necropsy-web
```

Then open <http://127.0.0.1:8080>. On first run the server creates an admin account and prints its generated username and password to the log, once.

## Layout

| Path | |
| --- | --- |
| `packs/` | Puzzle packs. |
| `crates/necropsy-pack` | Pack format library and the `pack-tool` authoring helper. |
| `crates/necropsy-web` | Web front end. |
| `docs/pack-format.md` | The pack format. |
| `docs/web.md` | Running and operating the web front end. |
| `docs/live.md` | Live puzzles: per-player servers on a wildcard domain. |
| `deploy/install-vm.sh` | One script that installs everything on a dedicated VM. |
| `docs/range.md` | Container-backed live puzzles: sandbox, setup, and how many players a VM holds. |
| `crates/necropsy-spawner` | Starts and reaps the sandboxed containers. |
| `range/` | Lab images and the image allowlist. |
| `docs/deploy.md` | Installing as a systemd user service. |
| `.env.example` | Every setting, documented. Copy to `.env` for local runs. |
| `deploy/` | Service unit template and `install.sh`. |
| `docs/docker.md` | Blue-green Docker deploys with Caddy, the sudo model, server setup. |
| `Dockerfile` | The release image. |
| `docs/ci-cd.md` | GitHub Actions: checks, and deploying to the server over SSH. |
| `.github/workflows/` | `ci.yml` and `deploy.yml`. |
| `docs/accounts.md` | Accounts, email, admin, and the security design. |

The packs contain answer hashes and explainers, so spoilers are in this repository.

## Adding a puzzle

1. Create `packs/<id>/` with `pack.toml`, `brief.md`, `files/` and `explainer.md`. Set `track` to `intro`, or to `daily` with a release `date` after the last daily.
2. Generate the answer file: `cargo run -q -p necropsy-pack --bin pack-tool -- hash <id> '<answer>' > packs/<id>/answer`
3. Check it loads: `cargo run -q -p necropsy-pack --bin pack-tool -- validate packs`

The site never runs anything from a pack. See [docs/pack-format.md](docs/pack-format.md), and [docs/live.md](docs/live.md) for live puzzles, where each player gets a server of their own.

## Checks

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## License

[GNU AGPL v3 or later](LICENSE). You can use, change and share this software, but anything you distribute, or run as a public network service, must be released under the same license with its source.
