# AGENTS.md

Rules for AI agents and contributors. They are short on purpose: follow them.

## Read before you change anything

1. Read `README.md`, then [`docs/design.md`](docs/design.md) (copy, layout and documentation standards).
2. Read the docs page for the area you are touching. Do not work from memory of how the code behaves; the docs and the code are the record.

| Area | Read |
| --- | --- |
| Pages, routes, the JSON API, player state, time, hints, stats, search | [`docs/web.md`](docs/web.md) |
| Accounts, login, two-step verification, abuse limits, the database | [`docs/accounts.md`](docs/accounts.md) |
| Puzzle packs and their format | [`docs/pack-format.md`](docs/pack-format.md) |
| Sealed packs, keys, release | [`docs/sealing.md`](docs/sealing.md) |
| Leaderboards, ranked attempts | [`docs/leaderboards.md`](docs/leaderboards.md) |
| The browser solver, `solve.browser.js` | [`docs/browser-solver.md`](docs/browser-solver.md) |
| The workspace: pcap viewer, decoder, request console, notebook | [`docs/toolkit.md`](docs/toolkit.md) |
| The Learn track and its practice puzzles | [`docs/learn.md`](docs/learn.md) |
| Discord link and discussion threads | [`docs/community.md`](docs/community.md) |
| Live puzzles, the Library, writing a sample | [`docs/live.md`](docs/live.md) |
| Container labs, the range VM, the spawner | [`docs/range.md`](docs/range.md) |
| Building and running the image | [`docs/docker.md`](docs/docker.md) |
| CI, deploys, secrets | [`docs/ci-cd.md`](docs/ci-cd.md), [`docs/deploy.md`](docs/deploy.md) |

## Write docs with every change

- A change that alters behaviour, or adds a feature, setting, route, puzzle-format option or operational step, updates the docs **in the same commit**. Not later, not in a follow-up.
- New setting: `.env.example` and the page for its area. New route or API: `docs/web.md`. New puzzle feature: `docs/pack-format.md` or `docs/live.md`. New lab or range behaviour: `docs/range.md`.
- A new feature gets documentation that covers what it does, how to turn it on, its limits, and what an operator does when it breaks.
- If the docs and the code disagree, fix whichever is wrong before you finish. Never leave a known mismatch.
- A new page in `docs/` is listed in `README.md` (a test checks this).
- Docs follow the copy rules in `docs/design.md`: state the fact, skip the pitch.

## Copy

No filler, no narration, no padding, in pages, forms, emails, puzzle briefs, explainers and docs. If deleting a sentence loses no fact, delete it. The rules, and the phrases that mark filler, are in [`docs/design.md`](docs/design.md); `no_filler_in_copy` fails the build on the worst of them.

## Tests and checks

- Every behaviour has a test, and a bug fix has a regression test that fails without the fix. Test the refusals as well as the success.
- Check what a player sees. For anything visual, look at it in a browser at 320 px and desktop width.
- Run before you commit, and fix what they report:

```bash
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
node --test crates/necropsy-web/tests-js/*.test.mjs
cargo run -q --bin pack-tool -- validate packs
cargo run -q --bin pack-tool -- check-gen packs && cargo run -q --bin pack-tool -- gen-build --check packs
cargo run -q --bin pack-tool -- check-sealed packs packs-sealed
cargo build -q --bin necropsy-web && cargo run -q --bin pack-tool -- solve-browser packs && node tools/browser-solver/selftest.mjs target/debug/necropsy-web toolkit/dist
deploy/test/common.sh && deploy/test/install-site.sh && deploy/test/install-range.sh && deploy/test/range-update.sh
deploy/bluegreen/test/run.sh && range/local-test.sh
```

## Git

- Work on the scratch branch you were given. When a change is done and its checks pass, push that branch and **open a pull request from it into `main`**, without being asked. If a pull request from the branch is already open, the push updates it and no second one is opened; say which one in your reply.
- **`dev` is for testing that needs a live server** (a lab on the range, the real domain, mail, a deploy). Push to `dev` only when the user asks for such a test, and say so when you do. Never open a pull request from `dev`, and never merge `dev` into `main` or into your scratch branch.
- Commit messages say what changed and why. Never commit secrets, `.env` files, database files, `packs-src/` or key files.
- Secrets go in files or the environment, never in command-line options.
