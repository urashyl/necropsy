# necropsy

Short puzzles built from real findings, such as malware, browser extensions and decrypted profile data. Each puzzle is a pack: a brief, some files, an answer and an explainer. The Learn track takes a beginner from a first packet capture to a 5/5 puzzle, a new daily puzzle is released each day, and the old ones stay playable. Every puzzle has a workspace with a pcap viewer, decoder, request console, Python notebook and hex viewer in the browser, so difficulty 1 to 3 needs nothing installed.

Demo site: <https://necropsy.urashyl.dev>

```bash
cargo run -p necropsy-web
```

Run `toolkit/fetch.sh` once first for the workspace's tools (see [docs/toolkit.md](docs/toolkit.md)). Then open <http://127.0.0.1:8080>. On first run the server creates an admin account and prints its generated username and password to the log, once.

## Layout

| Path | |
| --- | --- |
| `packs/` | Puzzle packs. |
| `crates/necropsy-pack` | Pack format library and the `pack-tool` authoring helper. |
| `crates/necropsy-web` | Web front end. |
| `docs/pack-format.md` | The pack format. |
| `docs/web.md` | Running and operating the web front end. |
| `docs/live.md` | Live puzzles: per-player servers on a wildcard domain. |
| `docs/design.md` | Copy, layout and documentation standards. |
| `docs/toolkit.md` | The workspace: pcap viewer, decoder, request console, notebook, hex viewer. |
| `toolkit/` | `fetch.sh` and the pinned versions of the toolkit's third-party builds. |
| `docs/learn.md` | The Learn track: modules, tool paths, checkpoints, and writing a module. |
| `docs/sealing.md` | Sealed packs: unreleased dailies encrypted in the repository, the keys, and when each is released. |
| `packs-sealed/` | Sealed packs and the public wrap keys. |
| `docs/moderation.md` | Proofs from the top of each ranked board, moderators, and the audit log. |
| `docs/leaderboards.md` | The casual and ranked leaderboards, ranked attempts and personal records. |
| `docs/browser-solver.md` | The browser solver: proves a daily of difficulty 1 to 3 can be solved in the workspace, for every player. |
| `tools/browser-solver/` | The browser solver's runner and its self-test. |
| `docs/generators.md` | Generated packs: per-player files and answers, the sandbox, the server secret. |
| `crates/necropsy-gen-sdk` | The library pack generators are written with. |
| `docs/community.md` | Discord: the link, and discussion threads for closed dailies. |
| `learn/` | The Learn modules. |
| `tools/learn/` | Builds the files of the Learn practice puzzles. |
| `AGENTS.md` | Rules for agents and contributors: read the docs first, write them with every change. |
| `deploy/install-site.sh` | Installs a site (production or dev) on its own VM. |
| `deploy/install-range.sh` | Installs the range, the VM that runs live-puzzle labs for the sites. |
| `docs/range.md` | Container-backed live puzzles: the range VM, its sandbox, setup, and how many players it holds. |
| `crates/necropsy-spawner` | Runs on the range VM: the sandboxed labs, for the sites. |
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

An unreleased daily is written in `packs-src/<id>/` (never committed) and sealed into `packs-sealed/` with `pack-tool seal`: see [docs/sealing.md](docs/sealing.md). Intro, Library and released packs are plaintext in `packs/`:

1. Create `packs/<id>/` with `pack.toml`, `brief.md`, `files/` and `explainer.md`. Set `track` to `intro` (a Learn practice puzzle, see [docs/learn.md](docs/learn.md)), `library`, or `daily` with a release `date` after the last daily. A daily can name the Learn modules it is a worked example for (`learn = [...]`).
2. For a generated pack (each player gets their own files and answer), write `generator/` instead of `files/` and the answer file: see [docs/generators.md](docs/generators.md). A generated daily of difficulty 1 to 3 also needs `solve.browser.js`: see [docs/browser-solver.md](docs/browser-solver.md). Otherwise, generate the answer file: `cargo run -q -p necropsy-pack --bin pack-tool -- hash <id> '<answer>' > packs/<id>/answer`
3. Check it loads: `cargo run -q -p necropsy-pack --bin pack-tool -- validate packs` (this also checks the Learn modules)
4. At difficulty 1 to 3, solve it in the workspace (`/w/<id>`) with nothing installed.

The site never runs anything from a pack. See [docs/pack-format.md](docs/pack-format.md), and [docs/live.md](docs/live.md) for live puzzles, where each player gets a server of their own.

## Checks

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
node --test crates/necropsy-web/tests-js/*.test.mjs
```

## License

[GNU AGPL v3 or later](LICENSE). You can use, change and share this software, but anything you distribute, or run as a public network service, must be released under the same license with its source.
