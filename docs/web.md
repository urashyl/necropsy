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
| `NECROPSY_CLIENT_IP_HEADER` | unset | Header your reverse proxy sets to the client address, such as `X-Real-IP`. Used for rate limiting. |
| `NECROPSY_SMTP_*`, `NECROPSY_DEV_MAIL_LOG` | unset | Email, see [accounts.md](accounts.md). |

On first run the server prints generated admin credentials to the log, see [accounts.md](accounts.md).

Packs are loaded and rendered once at startup. The server exits if any pack is invalid, so a bad pack never reaches production. Restart to pick up pack changes.

## Routes

| Route | |
| --- | --- |
| `/` | Today's puzzle. Packs cycle in order, one per UTC day. |
| `/p/{id}` | A puzzle by id. |
| `/p/{id}/files/{path}` | Raw file from the pack. Looked up by exact match against the loaded file list, never joined onto a filesystem path. |
| `/archive` | All puzzles. |
| `/format` | Renders `docs/pack-format.md`. |
| `POST /api/p/{id}/answer` | `{"answer": "...", "seconds"?: n, "hints"?: n}` returns `{"correct": bool, "explainer_html"?: string}`. The explainer is only included for a correct answer. For a signed-in user a correct answer is also saved, with the reported time and hints clamped to sane ranges. |
| `/register`, `/verify`, `/login`, `/logout`, `/forgot`, `/reset`, `/account`, `/email/confirm` | Accounts, see [accounts.md](accounts.md). |
| `/admin` | Admin only. |
| `GET /api/me/progress` | The signed-in user's solves, or `{"user": null}`. |
| `GET /api/p/{id}/explainer` | The explainer, for a signed-in user who has solved the puzzle. |
| `/healthz` | `ok`. |

## Player state

Without an account the server stores nothing about players. `app.js` keeps solves, hints used and active time in `localStorage` under `necropsy:v1`. The streak is the number of consecutive UTC days, ending today or yesterday, with at least one solve. Time only counts while the tab is visible.

A returning player's solved answer is kept locally and re-submitted on page load to bring the explainer back.

For a signed-in user the server also stores solves. On each page load `app.js` fetches `/api/me/progress`, uploads any solves that only exist in this browser (by re-submitting the saved answer), and merges both sets for the archive marks and the streak. On a new device the explainer comes from `/api/p/{id}/explainer`.

## Security notes

- Pack markdown is rendered with raw HTML converted to text. Highlighted code comes from syntect and is escaped. Everything else goes through maud, which escapes.
- The CSP allows only same-origin scripts, plus the Google Fonts stylesheet and font files. There is no inline script or style, so the theme bootstrap lives in `static/theme.js`.
- The answer endpoint caps the request body at 2 KiB and ignores answers over 512 bytes.
- Pages are sent with `Cache-Control: no-store` and `Referrer-Policy: same-origin`, since email links carry one-time tokens in the query string.
- The access log records method and path only, never query strings.
- Cross-site POSTs are rejected, see [accounts.md](accounts.md).
- Answer hashes and explainers are public in the repository. See the note in `pack-format.md`.

## Design

`static/app.css` uses the same tokens and fonts as urashyl.dev (Schibsted Grotesk and JetBrains Mono, the green palette, light and dark). Token colors for code are `--hl-*` and are defined for both themes.

## Not built yet

The CLI, packs 4 and 5 (multi-file, CLI-first), and deployment.

## Network use and the AGPL

The project is AGPL-3.0-or-later. Anyone who runs a modified version as a public service must offer its users the modified source. The footer's "Source" link is how this site does that, so a modified deployment should point it at its own source.
