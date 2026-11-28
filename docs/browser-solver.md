# Browser solver

A daily of difficulty 1 to 3 must be solvable in the browser, with the workspace (`/w/<id>`) and nothing installed. For a puzzle with one answer for everyone, a reviewer can check that by hand. For a generated pack every player's files and answer differ, so the pack carries a script, `solve.browser.js`, that solves it in the workspace, and the browser solver runs it for several players.

## What it checks

`pack-tool solve-browser` starts a site holding only that pack (a copy, its date set to yesterday so it is open to everyone), then for each player:

1. opens `/w/<id>` in a new Chromium context: a new signed-out player, so a new seed and new files;
2. runs the pack's script against the page;
3. types the answer the script returns into the answer form, submits it, and passes if the site says "Correct.".

The script gets the page and nothing else: not the seed, not the server secret, not the pack's other files, and the site it talks to never has the script. So the script can only find each player's answer the way that player would. Every player must solve it, or the pack fails.

## Which packs need one

| Pack | Needs `solve.browser.js` | Checked by |
| --- | --- | --- |
| A generated daily of difficulty 1 to 3 in `packs/` | yes: `pack-tool validate` fails without it | CI, on every push |
| A sealed daily of difficulty 1 to 3 | yes: `pack-tool seal` refuses without it | `pack-tool seal`, which runs it before sealing |
| Any other pack | no; if it has one, it is run | CI |

The site does not run it: the production image has no browser. A sealed pack is solved on the author's machine when it is sealed, and an admin reviewing a submission can run it again (below).

## Writing a script

`solve.browser.js` sits next to `pack.toml`. It exports one async function, which gets the helpers below and returns the answer:

```js
// packs/kerberoast/solve.browser.js
export default async function solve(t) {
  const out = await t.python(`import csv, io
from collections import defaultdict
from necropsy import file

rc4 = defaultdict(set)
for r in csv.DictReader(io.StringIO((await file("4769.csv")).decode())):
    if r["TicketEncryptionType"] == "0x17":
        rc4[r["TargetUserName"]].add(r["ServiceName"])
user = max(rc4, key=lambda u: len(rc4[u]))
print(user.split("@")[0])`);
  return out.trim().split("\n").pop();
}
```

| Helper | |
| --- | --- |
| `t.page` | The Playwright page, on `/w/<id>`. Use it for tools the helpers below don't cover: the pcap viewer, the decoder, the request console. Their tabs are `#tab-<tool>` and their panes `#pane-<tool>` ([toolkit.md](toolkit.md)). |
| `await t.tool(name)` | Opens a workspace tab: `files`, `pcap`, `decoder`, `console`, `notebook` or `hex`. |
| `await t.files()` | The file names the files pane lists. |
| `await t.python(code)` | Adds a notebook cell, types `code`, runs it, and returns what it printed. Throws if the cell fails. `from necropsy import file` reads a puzzle file, as for players. |

Solve it the way the brief and explainer expect a player to, with the tools a player has. Don't read the answer out of something a player can't see. Keep the script general: it runs against files from many seeds, so search for what the generator varies instead of hard-coding it.

## Running it

It needs Node 22, Playwright with Chromium, the toolkit's builds and a built site:

```bash
toolkit/fetch.sh
cargo build --bin necropsy-web --bin pack-tool
(cd tools/browser-solver && npm ci && npx playwright install chromium)
target/debug/pack-tool solve-browser packs kerberoast
```

| | |
| --- | --- |
| `pack-tool solve-browser <packs-dir> [id...] [--seeds N]` | Runs each named pack, or every pack in the directory with a script. 5 players by default. |
| `NECROPSY_WEB_BIN` | The site binary. Default: `necropsy-web` next to `pack-tool`. |
| `NECROPSY_TOOLKIT` | The toolkit's builds. Default: `toolkit/dist` in the repository. |
| `NECROPSY_CHROMIUM` | A Chromium to use instead of Playwright's. |
| `node tools/browser-solver/solve.mjs --pack <dir> --site-bin <bin> [--toolkit <dir>] [--seeds N] [--timeout S] [--keep]` | The runner itself. `--keep` leaves its site's directory (database, packs) for a look. |

A player gets 120 seconds. Each run prints one line per player and a summary:

```text
kerberoast: player 1: solved in 2.7 s
kerberoast: ok, 5 of 5 players solved it in the browser (5 different answers)
```

To check a submission as an admin: `pack-tool open <id>`, then `pack-tool solve-browser packs-src <id>`.

`node tools/browser-solver/selftest.mjs <necropsy-web> <toolkit-dir>` checks the solver itself refuses a wrong answer, a script that throws, one that never answers, an empty answer, and one player's answer given for another. CI runs it, then `solve-browser packs`, in its `browser` job.

## When it fails

| Output | Cause, and what to do |
| --- | --- |
| `FAILED: answered "...", Not it.` | The script found the wrong answer for that player's files. Run `pack-tool gen <packs> <id> <seed> out/` for a few seeds and compare what the script searches for with what the generator varies. |
| `FAILED: the notebook cell failed:` and a traceback | The Python in the script broke on that player's files. The traceback is the notebook's. |
| `FAILED: the notebook: Python is not installed on this server.` | The site has no toolkit builds: run `toolkit/fetch.sh`, or set `NECROPSY_TOOLKIT`. |
| `FAILED: no answer within 120 s` | The script waited for something that never appeared, usually a selector that is wrong. Run with `--keep` and open the page yourself. |
| `the site stopped` or `the site did not answer` | The site would not start with the pack. The last lines of its log follow; `pack-tool validate` on the pack usually says why. |
| `Cannot find module 'playwright'` | Run `npm ci` in `tools/browser-solver`. |
| `Executable doesn't exist` | Run `npx playwright install chromium` there, or set `NECROPSY_CHROMIUM`. |
