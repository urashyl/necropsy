# Web front end

`crates/necropsy-web` is an axum server that renders pages from a directory of packs. The pack format is in [pack-format.md](pack-format.md).

## Running

```bash
cargo run -p necropsy-web
```

| Variable | Default | |
| --- | --- | --- |
| `NECROPSY_ADDR` | `127.0.0.1:8080` | Listen address. |
| `NECROPSY_PACKS` | `packs` | Directory containing one subdirectory per pack. |
| `NECROPSY_BASE_URL` | `https://necropsy.urashyl.dev` | Origin used in links, emails and canonical URLs. Change it when the domain changes. Cookies are `Secure` when it starts with `https://`. |
| `NECROPSY_DB` | `necropsy.db` | SQLite file for accounts and solves. |
| `NECROPSY_SEALED`, `NECROPSY_MASTER_KEY_FILE`, `NECROPSY_MASTER_KEY` | `packs-sealed`, `master-key` next to the server secret, unset | Sealed packs and the master key that opens them. The key file is made if missing. See [sealing.md](sealing.md#on-the-site). |
| `NECROPSY_SERVER_SECRET_FILE` | `server-secret` next to `NECROPSY_DB` | The secret that keys generated packs' seeds and live flags. Made if missing. See [generators.md](generators.md#the-server-secret). |
| `NECROPSY_LEARN` | `learn` | The [Learn](learn.md) modules. The server exits if one is invalid. |
| `NECROPSY_TOOLKIT` | `toolkit/dist` | The workspace's third-party tools, from `toolkit/fetch.sh` (see [toolkit.md](toolkit.md)). Missing tools are reported in the log and on their tab. |
| `NECROPSY_LIVE_DOMAIN` | unset | Turns on [live puzzles](live.md) at `<label>.<domain>`. |
| `NECROPSY_LIVE_HOST_HEADER` | unset | Header with the original host, only if your proxy cannot pass `Host` through. |
| `NECROPSY_RANGE_URL`, `NECROPSY_RANGE_TOKEN` | unset | The range VM that runs container-backed [live puzzles](range.md), and this site's token there. Set both or neither. |
| `NECROPSY_TURNSTILE_SITE_KEY`, `NECROPSY_TURNSTILE_SECRET` | unset | Cloudflare Turnstile for registration and live instances, see [accounts.md](accounts.md#captcha). Set both or neither. |
| `NECROPSY_LOG_REQUESTS` | unset | `1` logs every request (method, path, status). Off by default. |
| `NECROPSY_CLIENT_IP_HEADER` | unset | Header your reverse proxy sets to the client address, such as `X-Real-IP`. Used for rate limiting. |
| `NECROPSY_TRUSTED_PROXIES` | unset | IPs or CIDR ranges allowed to supply that header, comma-separated. The header from any other connection is ignored. |
| `NECROPSY_SMTP_*`, `NECROPSY_DEV_MAIL_LOG` | unset | Email, see [accounts.md](accounts.md). |
| `NECROPSY_DISCORD_URL`, `NECROPSY_DISCORD_WEBHOOK`, `NECROPSY_DISCORD_GUILD_ID` | unset | Discord link and discussion threads for closed dailies, see [community.md](community.md). |

On first run the server prints generated admin credentials to the log, see [accounts.md](accounts.md).

Packs are loaded and rendered once at startup. The server exits if any pack is invalid, so a bad pack never reaches production. Restart to pick up pack changes.

## Routes

| Route | |
| --- | --- |
| `/` | Today's puzzle: the latest daily released by today (the viewer's own date, see [Days and timezones](#days-and-timezones)). Goes to `/learn` if none is out yet. |
| `/learn` | The [Learn](learn.md) modules by tier, the tool path choice, and every practice puzzle (the `intro` track). |
| `/learn/{id}` | A Learn module: the idea, the walkthrough for the chosen path, practice, checkpoint. `?path=browser\|terminal` shows that walkthrough. |
| `POST /learn/path`, `POST /learn/{id}/checkpoint` | Learn forms, see [learn.md](learn.md#routes). |
| `POST /api/learn/{id}/checkpoint`, `GET /api/me/learn` | Learn's JSON API, see [learn.md](learn.md#routes). |
| `/w/{id}` | The puzzle's [workspace](toolkit.md): its files and the in-browser tools, with the answer form, hints and clock. Unreleased dailies are "not found". |
| `/toolkit/{path}` | The toolkit's third-party files (Wiregasm, Pyodide, CyberChef), cached for good. See [toolkit.md](toolkit.md#how-the-files-are-served). |
| `/static/toolkit/{file}` | The workspace's own scripts, revalidated by ETag. |
| `GET /api/packs/wrap-key`, `GET /api/packs/keys`, `GET /api/packs/{id}/key` | The public key packs are sealed to, and released packs' keys. See [sealing.md](sealing.md#endpoints). |
| `GET /api/live/{label}/ws` | The request console's WebSocket to the player's own live instance. See [toolkit.md](toolkit.md#transport). |
| `/p/{id}` | A puzzle by id. Unreleased dailies are "not found". |
| `/p/{id}/files/{path}` | Raw file from the pack. Looked up by exact match against the loaded file list, never joined onto a filesystem path. For a [generated pack](generators.md), the asker's own file, `no-store`. |
| `/archive` | Released dailies, newest first. |
| `/leaderboard`, `/leaderboard/ranked` | The casual board, and a live daily's ranked board (`?pack=<id>`). See [leaderboards.md](leaderboards.md). |
| `POST /leaderboard/ranked/{pack}/{user}/hide`, `.../show` | Admin only: hide a ranked entry with a reason, or show it again. |
| `/library` | Puzzles outside the daily rotation. |
| `/card/{token}`, `/card/{token}.svg` | A shared solve card: the page, and the image (SVG, 1200 by 630). Public, no login, `noindex`. See [Solve cards](#solve-cards). |
| `/search` | `?q=words` and `?tag=name`. Puzzles whose title, tags, id or brief contain every word, best match first (title, then tags and id, then brief). With no query it lists every tag with its puzzle count. Only puzzles the player can open are searched, and explainers and hints never are, since they hold the answer. A puzzle's tags link here. |
| `/format` | Renders `docs/pack-format.md`. |
| `POST /api/p/{id}/answer` | `{"answer": "...", "seconds"?: n, "hints"?: n}` returns `{"correct": bool, "explainer_html"?: string, "seconds"?: n, "hints"?: n, "attempt"?: "ranked" | "personal"}`. The explainer is only included for a correct answer. A generated pack's answer is checked against the asker's own (see [generators.md](generators.md)). At most 10 answers per 2 seconds per account (or address, signed out); more get `429`. For a signed-in user a correct answer is also saved, with the server's own time and hints, and those two come back. The `seconds` and `hints` in the request are used only for a solve the server never timed (see [Time and hints](#time-and-hints)). On a live daily, `attempt` says which attempt the solve ended (see [leaderboards.md](leaderboards.md)). |
| `POST /api/p/{id}/beat` | `{"have": n, "peek"?: bool}`. The page is open: starts the clock on the first call and returns `{"user": bool, "seconds"?: n, "hints": n, "texts": [...], "solved": bool, "running": bool}`. |
| `POST /api/p/{id}/share` | Signed in. Turns sharing on for the player's solve of the puzzle and returns `{"url": ".../card/<token>", "text": "..."}`, the same address each time, and the text to send (see [Solve cards](#solve-cards)). `409` if the solve has no server-kept time, or there is no solve. `DELETE` turns it off (`204`). |
| `POST /api/p/{id}/hint` | `{"have": n}`. Reveals the next hint and returns the same shape. |
| `/register`, `/verify`, `/login`, `/login/verify`, `/logout`, `/forgot`, `/reset`, `/account`, `/account/mfa/*`, `/email/confirm` | Accounts, see [accounts.md](accounts.md). |
| `/admin` | Admin only. |
| `GET /api/me/progress` | The signed-in user's solves, or `{"user": null}`. |
| `GET /api/p/{id}/explainer` | The explainer, for a signed-in user who has solved the puzzle. |
| `/healthz` | `ok`. |

## Player state

Without an account the server stores nothing about players. `app.js` keeps solves, hints used and active time in `localStorage` under `necropsy:v1`, and passed Learn checkpoints under `necropsy:learn:v1`. The workspace's notebook keeps its cells under `necropsy:notebook:<id>`. The streak is the number of consecutive days (in the player's own timezone), ending today or yesterday, with at least one solve. A signed-out player's timer is the same rule kept in the browser: time since the page was first opened, up to 3 hours.

A returning player's solved answer is kept locally and re-submitted on page load to bring the explainer back.

For a signed-in user the server also stores solves. On each page load `app.js` fetches `/api/me/progress`, uploads any solves that only exist in this browser (by re-submitting the saved answer), and merges both sets for the archive marks and the streak. On a new device the explainer comes from `/api/p/{id}/explainer`.

## Time and hints

For a signed-in player the **server** keeps the time and the hints, so the browser cannot report them.

- **Time.** The clock starts the first time the player opens the puzzle (the page's first `/api/p/{id}/beat`). On a live puzzle it starts when the player starts their first instance instead, so reading the brief, asking for hints and waiting do not run it; restarting an instance does not reset it, and an admin's instance does not start anyone's clock. Until it starts, the time shows 0:00. The time is the wall-clock time since then, so closing the tab or letting the laptop sleep does not pause it: there is no way to stop the clock, find the answer and come back. It stops growing at **3 hours**. The page shows the server's number, ticks locally and re-syncs once a minute.
- **Hints.** The hint text is not in the page. `/api/p/{id}/hint` reveals the next one and counts it. A hint is revealed once for the account, so reloading or using another device shows the same ones, and asking for more than exist counts nothing.
- **Solving.** A correct answer is recorded with the server's time and hint count, whatever the browser sends, and the progress record is removed. A solved puzzle shows its recorded time and hints and counts nothing more.
- **Solves the server did not time.** A solve with no progress record is stored as the browser reported it and marked untimed. That is a solve made while signed out and uploaded after signing in (a browser can only be trusted about its own history), or an answer posted without opening the page. Untimed solves count toward "solved by" but not toward averages.
- **Signed-out players** have no server record: their time and hint count stay in the browser, and `/hint` returns hint text for the count the browser sends, without keeping anything. They can solve puzzles but do not appear in the averages.

Progress records are deleted 30 days after they were started.

## Solve cards

A solved puzzle can be shared with one button. After solving, the puzzle page (and the workspace) shows the text that will be sent, and **Share**: the system share sheet where there is one, otherwise it copies the text. Once a solve the server timed has been shared, it also offers **Save image** (a PNG drawn in the browser from the card's SVG) and **Stop sharing**.

The text says which puzzle, a row of squares, the time, the hints and the streak, and the link:

```text
necropsy Daily #4 · Answer the door
🟨🟩 3:12 · 1 of 2 hints · 5-day streak
https://necropsy.urashyl.dev/card/abcd2345efgh6789
```

- **Squares.** One per hint the puzzle has: 🟨 for each hint used, 🟩 for each one not needed. A puzzle without hints is one 🟩. They say how it went without saying anything about the answer, so the text is safe to post where others have not solved it yet.
- **Streak.** Days in a row, ending on the day of this solve, with a daily solved on its own release day (in the solver's calendar). Shown only for a daily solved on its day, and only from 2 days.
- **The link.** A server-timed solve shares its card (below), and the server makes the text, so its numbers are the server's. Any other solve (signed out, imported, or answered without opening the page) shares the same text with the puzzle's link, made by the browser from its own record; its streak is the browser's current one.

A card is a page and an image of the same facts, for the people the text is sent to:

- **Only server-timed solves.** A card needs a solve whose time and hints the server kept (see [Time and hints](#time-and-hints)), so a card cannot show numbers the player made up.
- **What it shows.** The puzzle's title and number, the time, the hints used of the total with their squares, the difficulty, the date (in the solver's own calendar), the streak and the site's host. With at least four other timed solves of the puzzle, also "Faster than N% of solvers". It never shows the player's name or email, or the answer or explainer. The page has **Play it**, and **New to this? Learn** for someone who has never played.
- **Who can open it.** Anyone with the link, no login. The link holds a random 16-character token, made when the player first shares that solve. Pages and images are sent with `X-Robots-Tag: noindex, nofollow`, and the page's title and description carry the summary, so a pasted link previews as text ("Who got in? solved in 3:12", "Solved in 3:12 with 1 hint.").
- **Turning it off.** **Stop sharing** removes the token: the page and image return 404 at once. Sharing again makes a new token, so the old link stays dead. Deleting the account deletes the solves and their cards.
- **Not an image preview.** Link previews on chat sites use a PNG, and the card is an SVG, so previews show text, not the card. The card is shown on its page and saved as a PNG by the player.

## Statistics

Each puzzle page and list row shows how many players solved it and, from the timed solves, the average time and the average number of hints. These are plain aggregates over all signed-in players and never show who. They are recalculated at most every 30 seconds and right after someone solves something. The averages only include solves whose time the server kept, so they start from when this feature was added.

## Security notes

- Pack markdown is rendered with raw HTML converted to text. Highlighted code comes from syntect and is escaped. Everything else goes through maud, which escapes.
- The CSP allows only same-origin scripts, plus the Google Fonts stylesheet and font files. There is no inline script or style, so the theme bootstrap lives in `static/theme.js`. The workspace page adds a same-origin frame and workers; its workers and CyberChef have policies of their own (see [toolkit.md](toolkit.md#security)). A handler that sets its own `Content-Security-Policy` or `X-Frame-Options` keeps it; every other response gets the site's.
- The answer endpoint caps the request body at 2 KiB and ignores answers over 512 bytes.
- Pages are sent with `Cache-Control: no-store` and `Referrer-Policy: same-origin`, since email links carry one-time tokens in the query string.
- Requests are not logged unless you set `NECROPSY_LOG_REQUESTS=1`; then the log records method, path and status, never query strings.
- Cross-site POSTs are rejected, see [accounts.md](accounts.md).
- Answer hashes and explainers are public in the repository. See the note in `pack-format.md`.

## Design

`static/app.css` uses the same tokens and fonts as urashyl.dev (Schibsted Grotesk and JetBrains Mono, the green palette, light and dark). Token colors for code are `--hl-*` and are defined for both themes.

## Not built yet

The CLI, packs 4 and 5 (multi-file, CLI-first), and deployment.

## Network use and the AGPL

The project is AGPL-3.0-or-later. Anyone who runs a modified version as a public service must offer its users the modified source. The footer's "Source" link is how this site does that, so a modified deployment should point it at its own source.

## Days and timezones

A puzzle comes out on its `date` in the player's own timezone, like Wordle, so "today" changes at local midnight. The browser keeps its UTC offset in a `necropsy_tz` cookie (minutes ahead of UTC, set by `theme.js`; the page reloads once if it changed), and the server turns the real clock into the viewer's calendar day for each request.

The server never trusts that offset further than a real timezone: it is clamped to UTC-12 to UTC+14, so the furthest ahead anyone can get is the date where it is already tomorrow, a few hours early, and never days. A forged or garbage cookie is clamped or treated as UTC. Everything that reveals a puzzle (its page, files, answer check and explainer) uses the same rule, so there is no other way to read one early.

The clock is the machine's epoch time (`SystemTime`), plus the viewer's clamped offset, in whole days. The server's own timezone setting (`TZ`, `/etc/localtime`, a container's zone) is never read, so the host can be in any zone and nothing changes. A test (`no_server_timezone_is_used`) fails if code that reads local time is added.

A solve records the solver's offset at the time, so it counts as "on time" for the day it was solved in the solver's own calendar.
