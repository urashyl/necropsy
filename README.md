# necropsy

Short puzzles built from real findings, such as malware, browser extensions and decrypted profile data. Each puzzle is a pack: a brief, some files, an answer and an explainer.

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
| `docs/deploy.md` | Installing as a systemd user service. |
| `.env.example` | Every setting, documented. Copy to `.env` for local runs. |
| `deploy/` | Service unit template and `install.sh`. |
| `docs/docker.md` | Blue-green Docker deploys, rollback, server setup. |
| `Dockerfile` | The release image. |
| `docs/ci-cd.md` | GitHub Actions: checks, and deploying to the server over SSH. |
| `.github/workflows/` | `ci.yml` and `deploy.yml`. |
| `docs/accounts.md` | Accounts, email, admin, and the security design. |

The packs contain answer hashes and explainers, so spoilers are in this repository.

## Adding a puzzle

1. Create `packs/<id>/` with `pack.toml`, `brief.md`, `files/` and `explainer.md`.
2. Generate the answer file: `cargo run -q -p necropsy-pack --bin pack-tool -- hash <id> '<answer>' > packs/<id>/answer`
3. Check it loads: `cargo run -q -p necropsy-pack --bin pack-tool -- validate packs`

Packs must be inert text or data. See [docs/pack-format.md](docs/pack-format.md).

## Checks

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## License

[GNU AGPL v3 or later](LICENSE). You can use, change and share this software, but anything you distribute, or run as a public network service, must be released under the same license with its source.
