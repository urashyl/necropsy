## Pack format

A puzzle is a pack: a plain directory. The web site and the CLI read the same pack.

```text
phone-home/
  pack.toml       metadata
  brief.md        the question
  files/          the excerpt, or a file tree
  answer          sha256 of the normalized answer
  explainer.md    the real finding, shown after solving
```

### pack.toml

| Key | Type | Notes |
| --- | --- | --- |
| `id` | string | Slug, unique. Lowercase letters, digits and `-`, at most 40 characters. Used in URLs. |
| `track` | string | `intro` or `daily`, see below. Required. |
| `date` | string | `YYYY-MM-DD` (UTC) release date. Required for `daily`, not allowed for `intro`. Unique among dailies. |
| `number` | integer | Display number and order within its track. Unique within the track. |
| `title` | string | |
| `difficulty` | integer | 1 (easy) to 5 (hard). |
| `modes` | list | `web`, `cli`. Defaults to `["web"]`. |
| `tags` | list | Free text, shown on the puzzle page. |
| `hints` | list | Revealed one at a time, in order. |

Unknown keys are rejected.

### Tracks

There are two tracks of puzzles.

- **`intro`** stages are always available, in order. They teach the techniques the dailies assume, and the last one leads on to today's puzzle.
- **`daily`** puzzles have a release `date`. The latest one released is today's puzzle, earlier ones stay playable from the archive, and later ones are hidden everywhere: their page, files, answer check and explainer all answer "not found" until their day, so nothing can be read early. To add a daily, give it a date after the last one.

Streaks count only dailies solved on their own release day, so replaying old puzzles or the intro stages does not extend one.

### answer

The file holds one line, `sha256:` followed by 64 hex digits. The hash input is `necropsy:v1:<id>:<normalized answer>`.

Before hashing, an answer is normalized: surrounding whitespace and one layer of quotes or backticks are removed, runs of whitespace collapse to one space, and the text is lowercased. `Sync.PagePilot-CDN.invalid ` and `` `sync.pagepilot-cdn.invalid` `` are the same answer.

To generate the line for a new pack:

```bash
cargo run -q -p necropsy-pack --bin pack-tool -- hash <id> '<answer>' > packs/<id>/answer
```

The hash is stored in a public repository, so it only protects against casual spoilers. A short or guessable answer can be brute-forced from it, and `explainer.md` is public too. Prefer answers that are long or specific, such as a hostname, a path or a decrypted value.

### files/

Everything under `files/` is shown or handed to the player. Files are text or data only and are never executed.

**Packs never contain working malware.** A puzzle about a real sample uses what an analyst works from: file and registry activity logs, notes, message headers, decoded strings and indicators, or a short defanged snippet. Hosts use the reserved `.invalid` domain. When an artifact is rebuilt from published analyses rather than captured, its first lines say so and name the sources, and the explainer links them.

The loader rejects:

- symlinks and special files
- path components other than plain names, so no `..`, no absolute paths and no backslashes
- files over 1 MiB, packs over 8 MiB, more than 256 files, or nesting deeper than 8 levels

The web site shows text files with line numbers and highlighting by file extension. Binary files are offered as downloads.

### explainer.md and brief.md

Markdown. Raw HTML is removed. The explainer is only sent to the browser after a correct answer.

### Pack hash

`pack-tool validate packs` prints each pack's hash: SHA-256 over every file in the pack directory, ordered by path, with each path and length included. The CLI will compare it with a published value before unpacking a pack.

```bash
cargo run -q -p necropsy-pack --bin pack-tool -- validate packs
```
