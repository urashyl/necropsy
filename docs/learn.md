# Learn

`/learn` is a course from a first packet capture to the level a 5/5 daily needs. It is made of modules in `learn/`, each mapped to a difficulty tier, so the path runs from tier 1 to tier 5:

| # | Module | Tier | Practice |
| --- | --- | --- | --- |
| 1 | Reading a pcap | 1 | `learn-first-capture` |
| 2 | HTTP and C2 patterns | 1 | `learn-beacon` |
| 3 | curl by hand | 2 | `learn-knock` (live) |
| 4 | Encodings: base64, hex, URL and XOR | 2 | `learn-layers` |
| 5 | Crypto basics | 3 | `learn-aes-config` |
| 6 | Strings and binary triage | 3 | `learn-string-table` |
| 7 | Scripting the boring parts | 3 | `learn-replay` (live) |
| 8 | To the terminal | 4 | modules 1 and 3 again, with local tools |
| 9 | Answering the door | 4 | `learn-door` (live) |
| 10 | Full intrusions | 5 | Library: `ransom-recovery`, `implant-lab`, `elf-lab` |

Each module page has, in order: **the idea** (the concept, tool-independent), a **walkthrough** for the player's tool path, the **practice** puzzles (and more at the same level, and worked examples), and a **checkpoint**.

## Tool paths

A player chooses a path on `/learn`:

- **Browser:** walkthroughs use the [workspace](toolkit.md), with nothing installed. Each tool shows its terminal equivalent. Module 8 moves the player to local tools before tier 4, because difficulty 4 and 5 need them.
- **Terminal:** walkthroughs use Wireshark, tshark, curl and Python from module 1, with install steps for Linux, macOS and Windows.

Both walkthroughs of a module cover the same concepts and the same practice puzzle. The choice is a preference, not progress: switching shows the other walkthrough and changes nothing else. Checkpoints accept answers from either path. A module page can show the other walkthrough once with `?path=browser` or `?path=terminal`.

The choice is kept on the account when signed in (`users.learn_path`) and in a `necropsy_path` cookie (one year, `SameSite=Lax`) either way. The account's choice wins. With no choice, module pages show the browser path and say so.

## Checkpoints

A checkpoint is a few questions in `module.toml`. It is passed when every answer is right. The form is a plain HTML post (`POST /learn/<id>/checkpoint`) and works without JavaScript; a wrong answer shows the question's hint.

- **Signed in:** a pass is stored on the account (`learn_passed`). Deleting the account deletes it.
- **Signed out:** nothing is stored on the server. `app.js` keeps the passed module and its answers in the browser (`necropsy:learn:v1`), and after the player signs in sends them to `POST /api/learn/<id>/checkpoint`, which checks them again before recording them, the way solves are uploaded.

Learn progress is on the honour system, like the casual side of the site: the answers are in the repository. It is for the learner.

When every module of a tier is passed, `/learn` marks the tier **Complete**.

## Routes

| Route | |
| --- | --- |
| `/learn` | The modules by tier, the path choice, and every practice puzzle. |
| `/learn/<id>` | A module. `?path=browser|terminal` shows that walkthrough. |
| `POST /learn/path` | Form: `path` (`browser` or `terminal`), `back` (a `/learn` page to return to; anything else returns to `/learn`). Sets the cookie, and the account's choice when signed in. |
| `POST /learn/<id>/checkpoint` | Form: `a0`, `a1`, ... in question order. Returns the module page with each answer marked. |
| `POST /api/learn/<id>/checkpoint` | `{"answers": [...]}` returns `{"passed": bool, "results": [bool, ...]}`, and records a pass for a signed-in player. |
| `GET /api/me/learn` | `{"user", "path", "passed": [ids], "modules": [ids]}`. |

## Module format

```text
learn/
  01-reading-a-pcap/
    module.toml
    concept.md      the idea
    browser.md      walkthrough with the workspace
    terminal.md     walkthrough with local tools
```

Directory names only sort; the `number` in `module.toml` orders the modules.

```toml
id = "reading-a-pcap"          # slug, used in the URL; a-z, 0-9 and -
number = 1                     # order, unique
tier = 1                       # 1 to 5; never lower than the module before
title = "Reading a pcap"
summary = "One line, shown on /learn and as the page description."
practice = ["learn-first-capture"]   # pack ids; the first is the module's own
more = ["intro-log"]                 # optional: more puzzles at this level

[[checkpoint]]
question = "Which display filter shows only HTTP requests?"
kind = "filter"                # text (default), filter or curl
accept = ["http.request"]      # any of these is right
hint = "Shown after a wrong answer."

[[checkpoint]]
question = "Write a curl command that ..."
kind = "curl"
curl = "curl -H 'X-Key: abc' 'https://c2.example.test/status?id=7'"
```

| `kind` | An answer is right when |
| --- | --- |
| `text` | It equals an `accept` entry after the puzzle-answer normalization (case, surrounding quotes, runs of spaces). |
| `filter` | It equals an `accept` entry as a display filter: spaces and quotes ignored, `eq`/`ne`/`and`/`or` read as `==`/`!=`/`&&`/`||`. |
| `curl` | It is a curl command (the console's subset, see [toolkit.md](toolkit.md#supported-curl)) that sends the request in `curl`: same method, host, path and body, the same query parameters in any order, and at least its headers with the same values. Extra headers are fine. Use a made-up host such as `c2.example.test`. |

Rules, checked at startup (the server will not start) and by `pack-tool validate packs`:

- Every practice and `more` id is a pack, and none is a daily (a daily is hidden until its date).
- Each `text` or `filter` question accepts at least one answer; each `curl` question's `curl` parses.
- Module ids and numbers are unique, and tiers never go down.
- `concept.md`, `browser.md` and `terminal.md` exist and are not empty.

A practice puzzle that is hidden on a site (a live one without `NECROPSY_LIVE_DOMAIN`, a container lab without the range) is listed with a note saying so.

| Setting | Default | |
| --- | --- | --- |
| `NECROPSY_LEARN` | `learn` | The modules directory. A missing directory means no modules: `/learn` still lists the practice puzzles. |

## Practice puzzles

Practice puzzles are packs on the `intro` track, numbered in module order, with `learn` in their tags. Their files are made by `tools/learn/build.py` (captures with `tools/learn/pcaplib.py`, which writes real TCP handshakes, sequence numbers and checksums). It is deterministic, and prints each answer for `pack-tool hash`:

```bash
python3 tools/learn/build.py
```

It needs the `cryptography` package for the AES config. Change a practice file by changing the script and running it again, not by editing the file, and check the answers it prints against the `answer` files. `learn-replay`'s `live.toml` is generated by the same script.

The first practice puzzles are 1/5: a small capture with one thing to find. Each practice pack's hints end with near-answers, and where a CyberChef recipe does the work, the last hint opens the decoder with it.

## Worked examples

A daily lists the modules it is a worked example for in `pack.toml`:

```toml
learn = ["encodings", "crypto-basics"]
```

Once it has closed (its date is over in every timezone, see [community.md](community.md#when-a-daily-is-discussed)), it is listed under **Worked examples** on those modules' pages. Its explainer is shown to players who solve it.

## Writing a module

- Copy rules apply ([design.md](design.md)): state the fact, skip the pitch. `no_filler_in_copy` scans every module file.
- Write `concept.md` without naming a tool's buttons; put the steps in the walkthroughs.
- Both walkthroughs end at the practice puzzle. Every command in a terminal walkthrough must work as written; every button named in a browser walkthrough must exist in the workspace.
- Checkpoint questions test the concept and accept every reasonable form of the answer. Run the module's page with a wrong and a right answer for each.
- A test (`the_repository_modules_load`) checks that each question's first accepted answer, or its `curl`, is right.

## When something breaks

| Symptom | Cause, and what to do |
| --- | --- |
| The server will not start: "cannot load the Learn modules" | A module file is invalid; the message names the file. `pack-tool validate packs` shows the same error. |
| A module says its practice needs live puzzles | Set `NECROPSY_LIVE_DOMAIN` (see [live.md](live.md)), or the range for container labs. |
| A signed-out player's progress is gone | It is kept in that browser's localStorage only; clearing site data removes it. Signing in keeps it on the account. |
