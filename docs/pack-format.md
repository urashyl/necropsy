## Pack format

A puzzle is a pack: a plain directory. The web site and the CLI read the same pack.

```text
phone-home/
  pack.toml       metadata
  brief.md        the question
  files/          the excerpt, or a file tree
  answer          sha256 of the normalized answer
  live.toml       live puzzles only, see live.md and range.md
  live/           live puzzles only, files a scenario can send
  generator/      generated packs only: the generator's source, see generators.md
  generator.wasm  generated packs only: built from generator/
  explainer.md    the real finding, shown after solving
```

### pack.toml

| Key | Type | Notes |
| --- | --- | --- |
| `id` | string | Slug, unique. Lowercase letters, digits and `-`, at most 40 characters. Used in URLs. |
| `track` | string | `intro`, `daily` or `library`, see below. Required. |
| `date` | string | `YYYY-MM-DD` release date: the puzzle comes out on that calendar day in each player's own timezone (see [web.md](web.md#days-and-timezones)). Required for `daily`, not allowed for `intro` or `library`. Unique among dailies. |
| `number` | integer | Display number and order within its track. Unique within the track. |
| `title` | string | |
| `difficulty` | integer | 1 (easy) to 5 (hard). |
| `modes` | list | `web`, `cli`, `live`. Defaults to `["web"]`. A pack with `live` is a [live puzzle](live.md): it has a `live.toml` and no `answer` file. |
| `tags` | list | Free text, shown on the puzzle page. |
| `hints` | list | Revealed one at a time, in order. See [Hints](#hints). |
| `answer_in_files` | bool | Generated packs only: the answer is meant to be read off the files as it is (a user name in a log), so `check-gen` allows it there. See [generators.md](generators.md#rules). |
| `learn` | list | Learn module ids this puzzle is a worked example for, shown on those modules once a daily has closed. See [learn.md](learn.md#worked-examples). |

Unknown keys are rejected.

### Sealed packs

An unreleased daily is not committed as a directory: it is sealed into `packs-sealed/<id>.sealed` with `pack-tool seal`, and the site opens it with its master key. Inside it is an ordinary pack directory. See [sealing.md](sealing.md).

### Tracks

There are three tracks of puzzles: `intro`, `daily` and `library`.

- **`intro`** stages are always available, in order. They are the [Learn](learn.md) track's practice puzzles: each module names its own, numbered in module order. The last one leads on to today's puzzle. A live `intro` pack is hidden, like any live pack, on a site without live puzzles; the "Intro N of M" count skips hidden ones.
- **`daily`** puzzles have a release `date`. The latest one released is today's puzzle, earlier ones stay playable from the archive, and later ones are hidden everywhere: their page, files, answer check and explainer all answer "not found" until their day, so nothing can be read early. To add a daily, give it a date after the last one.

- **`library`** puzzles are not part of the daily rotation. They are always available, in order, with no release `date` (a date is an error), listed on the Library page, not in the archive, and they never become today's puzzle. They can be any kind of puzzle, [live](live.md) or not, so the Library is where a collection grows without waiting for a date. A new one can be played, and tried by the admin, the moment it is deployed. (A live puzzle that should be one day's puzzle is just a `daily` with the `live` mode: there is no separate place for live puzzles.)

Streaks count only dailies solved on their own release day, so replaying old puzzles or the intro stages does not extend one.

### Generated packs

A pack with `generator.wasm` (and its source in `generator/`) is **generated**: each player's files and answer come from the generator, run with their own seed. It has no `answer` file, and `files/` holds only what every player shares, if anything. It cannot also be live. The generator's local build output, `generator/target/`, is skipped. See [generators.md](generators.md).

### answer

The file holds one line, `sha256:` followed by 64 hex digits. The hash input is `necropsy:v1:<id>:<normalized answer>`.

Before hashing, an answer is normalized: surrounding whitespace and one layer of quotes or backticks are removed, runs of whitespace collapse to one space, and the text is lowercased. `Sync.PagePilot-CDN.invalid ` and `` `sync.pagepilot-cdn.invalid` `` are the same answer.

To generate the line for a new pack:

```bash
cargo run -q -p necropsy-pack --bin pack-tool -- hash <id> '<answer>' > packs/<id>/answer
```

The hash is stored in a public repository, so it only protects against casual spoilers. A short or guessable answer can be brute-forced from it, and `explainer.md` is public too. Prefer answers that are long or specific, such as a hostname, a path or a decrypted value.

### files/

Everything under `files/` is shown or handed to the player. The site never executes a pack file.

Real samples are welcome: captured logs, scripts, loaders, credential and cookie loggers, kit source, whatever the puzzle is about. The site never runs anything from a pack; it only shows it or hands it over. What it does make sure of is that no one runs a sample by accident:

- Text files are shown with line numbers and served as `text/plain`.
- Anything that is not valid text is served as `application/octet-stream` with `Content-Disposition: attachment`.
- Give an executable sample a harmless name (`loader.exe.sample`, `payload.js.txt`), or ship it inside a password-protected archive, so it is never one double-click from running. Say what it is in the brief.

The loader rejects:

- symlinks and special files
- path components other than plain names, so no `..`, no absolute paths and no backslashes
- files over 1 MiB, packs over 8 MiB, more than 256 files, or nesting deeper than 8 levels

The web site shows text files with line numbers and highlighting by file extension. Binary files are offered as downloads.

### Hints

Hints go from a nudge to nearly the answer. A hint can end with a link that opens the [workspace](toolkit.md)'s decoder with a CyberChef recipe:

```toml
hints = [
  "After base64 you have %-escapes, then hex, then XOR with K. {decoder: From_Base64('A-Za-z0-9+/=',true,false)URL_Decode(true)From_Hex('Auto')}",
]
```

The recipe is everything from `{decoder:` to the hint's last `}`, in CyberChef's own recipe syntax (copy it from CyberChef's URL after `recipe=`, decoded). It must end the hint and be printable ASCII; the loader rejects it otherwise. The page shows the text before it, then **Open in the decoder**.

### Files in the workspace

Every file is also available in the puzzle's workspace (`/w/<id>`): `.pcap`, `.pcapng` and `.cap` files open in the pcap viewer, text files can be sent to the decoder, and any file opens in the hex viewer and the notebook. A capture for a difficulty 1 to 3 puzzle must open there: classic pcap or pcapng, Ethernet, HTTP only. `tools/learn/pcaplib.py` writes such captures.

### explainer.md and brief.md

Markdown. Raw HTML is removed. The explainer is only sent to the browser after a correct answer.

### Pack hash

`pack-tool validate packs` prints each pack's hash: SHA-256 over every file in the pack directory, ordered by path, with each path and length included. The CLI will compare it with a published value before unpacking a pack.

```bash
cargo run -q -p necropsy-pack --bin pack-tool -- validate packs
```
