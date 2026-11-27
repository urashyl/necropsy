# Generated packs

A generated pack has a **generator** instead of fixed files and an answer: a small program that turns a seed into one player's files and their expected answer. Each player gets their own seed, so their files lead to an answer that is theirs alone. Sharing the method is fine, since that is what the puzzle teaches; sharing the answer string does nothing for anyone else.

The converted dailies are `dns-exfil`, `kerberoast`, `lockfile` and `password-spray`.

## How it works

1. **A seed per player.** The first time a signed-in player opens the puzzle, the site stores a random seed for them and that pack (`gen_seeds`). A signed-out browser gets a random id in the `necropsy_anon` cookie (`HttpOnly`, `SameSite=Lax`, one year), and its seed is made from that.
2. **Keyed with the server secret.** The generator is never given the stored seed. It gets `HMAC-SHA-256(server secret, pack id, seed)`. Someone with a copy of the database but not the secret cannot work out anyone's files or answer.
3. **Run in a sandbox** (below), which returns the player's files and answer.
4. **Shown and checked per player.** The puzzle page, the workspace and `/p/<id>/files/<path>` show the asker's own files, marked "These files are yours". `POST /api/p/<id>/answer` compares the answer with the asker's own. Outputs are cached in memory (512 entries), and generating takes 5 to 40 ms.

**Signing in keeps the files.** When a signed-out browser signs in, the account takes that browser's seed if it has none yet, so the files do not change. An answer is accepted if it is right for the account's seed or for this browser's, so a puzzle solved signed out still counts when `app.js` uploads it.

**Copying between players** needs both the answer and the other player's browser id or account. That is collusion, not copying an answer. The casual board is on the honour system anyway; the ranked board uses live puzzles.

## The sandbox

A generator is WebAssembly (`generator.wasm`), run by [wasmi](https://github.com/wasmi-labs/wasmi), an interpreter with no JIT:

- **No imports.** A module that imports anything is refused before it runs. It has no clock, no randomness but the seed, no files, no network and no host calls, which also makes it deterministic.
- **Fuel:** at most 500 million instructions, about 1 to 2 seconds. Then it stops with "ran out of fuel".
- **Memory:** at most 64 MiB of linear memory and one memory, enforced on every grow.
- **Output checks:** at most 64 files, 1 MiB each and 8 MiB in all, with plain relative paths (no `..`, absolute paths, backslashes or control characters) and no duplicate paths. The answer is non-empty and at most 512 bytes.
- **Its own process.** The site runs it as `necropsy-web generate`, a child of itself, with an empty environment, `/` as its directory, and limits it sets on itself before reading anything: 10 CPU seconds, 1 GiB of address space, 16 open files, no file writes, no core dumps. The module and the 32-byte seed go in on stdin; the output comes back on stdout. The parent kills it after 10 seconds, then decodes and checks the output again. The server secret, the database and the sessions are never in the child.

## Writing a generator

```text
packs/<id>/
  pack.toml          no `answer` file; files/ only for what every player shares
  brief.md
  explainer.md       explains the method; never names an answer
  generator/
    Cargo.toml       a crate of its own (an empty [workspace] table), cdylib + rlib
    Cargo.lock
    src/lib.rs       the generator, and its own tests
    template/...     files with {{PLACEHOLDERS}}, if it fills a template
  generator.wasm     built from generator/ by pack-tool, committed
```

Copy `packs/kerberoast/generator/` as a start. The SDK is `crates/necropsy-gen-sdk`:

```rust
use necropsy_gen_sdk::{export, names, template, Gen};

const TEMPLATE: &str = include_str!("../template/4769.csv");

fn generate(g: &mut Gen) {
    let user = names::user(&mut g.rng);              // "k.morgan", from the seed
    let csv = template::render(TEMPLATE, &[("ATTACKER", &user)]);
    g.file("4769.csv", csv);                         // a file this player gets
    g.answer(user);                                  // the answer it leads to
}

export!(generate);
```

| Module | |
| --- | --- |
| `Gen` | `rng`, `file(path, bytes)`, `answer(text)`; in tests `file_data(path)` and `answer_text()`. |
| `Rng` | `below`, `range`, `pick`, `shuffle`, `bytes`, `string(alphabet, n)`, `hex(n)`, `flag(prefix, n)`. Seeded only from the player's seed. |
| `template::render` | Fills `{{NAME}}`; panics on a placeholder with no value, so `check-gen` catches a typo. |
| `encode` | `base64`, `base32` (lowercase, as DNS labels use it), `hex`, `xor`. |
| `names` | `user`, `distinct_users(n)`: made-up user names. |
| `pcap` | `Capture` with `http(...)` and `dns(...)`, `request(...)`, `response(...)`: real captures, as in `tools/learn/pcaplib.py`. |

A generator may depend on other crates that build for `wasm32-unknown-unknown` without imports (RustCrypto's `aes`, `sha2` and `hmac` do), and must not use anything that needs the OS: a module that imports `wasi` or `env` is refused.

### Rules

- **The answer must differ per seed.** `check-gen` tries 24 seeds and refuses a generator that gives fewer than 23 different answers. Vary everything that identifies the answer, not just the answer: in `kerberoast` every user name and workstation changes, or "the user at .30" would be a shareable answer.
- **Never in the pack's text.** No generated answer may appear in `brief.md`, `explainer.md`, `pack.toml`, `files/` or `generator/template/`. Lists of candidate values belong in `generator/src/`, which is exempt. The explainer explains the method and says each player's answer differs.
- **Not in the files as it is, unless it should be.** For an answer that is meant to be read off the files (a user name in a log), set `answer_in_files = true` in `pack.toml`. Otherwise `check-gen` refuses files that contain the answer as it is: a decoded value must be encoded in them.
- **Deterministic.** The same seed must give the same bytes, which comes from the sandbox. Do not hash a pointer or iterate a `HashMap` into the output.

## Tools

```bash
cd packs/<id>/generator && cargo test                  # the generator's own tests
cargo run -q --bin pack-tool -- gen-build packs <id>   # build generator.wasm
cargo run -q --bin pack-tool -- gen packs <id> alice   # files for the seed "alice", in target/gen/<id>/alice/; prints the answer
cargo run -q --bin pack-tool -- check-gen packs        # every generator, 24 seeds each
cargo run -q --bin pack-tool -- gen-build --check packs
```

`gen-build` builds with the toolchain pinned in `packs/rust-toolchain.toml` (rustup installs it and the `wasm32-unknown-unknown` target on first use), into `target/generators`, with paths remapped. The same source then builds the same bytes on any machine, which `gen-build --check` (in CI) relies on: a committed `generator.wasm` must be what its source builds, so a reviewer can read the source and trust the module. After changing a generator, run `gen-build` and commit both. To move to a newer toolchain, change `packs/rust-toolchain.toml` and rebuild every generator in the same commit.

`pack-tool gen` and `check-gen` use a fixed key of their own, not the site's secret, so their output is never a real player's.

## The server secret

| Setting | Default | |
| --- | --- | --- |
| `NECROPSY_SERVER_SECRET_FILE` | `server-secret` next to `NECROPSY_DB` | A file holding the secret: at least 32 characters. Created, random, with mode 0600, if it does not exist. |

It keys every generated seed and every live flag (see [live.md](live.md)). Keep it out of the repository and out of any copy of the database you share. The Docker image keeps it in the data volume (`/data/server-secret`) and the installed service in its state directory, beside the database.

**Changing it** changes every player's generated files and answers at once, and the flag of every live instance made since flags were keyed. A player in the middle of a puzzle gets new files, and an answer worked out from the old ones is refused. Change it only after a leak, and say so to players. Copying the database to a new server without the secret has the same effect: copy both.

## When something breaks

| Symptom | Cause, and what to do |
| --- | --- |
| The puzzle page says "This puzzle's files could not be made" | The generator failed. The log says why, as "generator for <id> failed: ...". "ran out of fuel" or "did not finish in time" means it is too slow for the limits; anything else is a bug in the generator. Reproduce it with `pack-tool gen packs <id> <any seed>`, fix it and redeploy. |
| "cannot start the generator process" in the log | The server could not run its own binary: check that the binary at the path in the log exists and is executable, and that the service is not denied `fork`/`exec` by a sandbox profile. |
| The server will not start: "the server secret ... is shorter than 32 characters" | The secret file was replaced with something short. Restore it from a backup; a new one changes every player's answers (above). |
| A player reports their answer stopped working | Was the secret changed, or the database moved without it? If so, their files changed too; they need to work from the files they see now. |
| CI fails at `gen-build --check` | The committed `generator.wasm` is not what its source builds. Run `pack-tool gen-build packs <id>` with the pinned toolchain and commit the result. |
| CI fails at `check-gen` | The message names the seed and the rule. Too few different answers: vary more. Answer in a file: encode it, or set `answer_in_files` if it is meant to be read there. |
