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
| `NECROPSY_BASE_URL` | `https://necropsy.urashyl.dev` | Origin used in share links and canonical URLs. Change it when the domain changes. |

Packs are loaded and rendered once at startup. The server exits if any pack is invalid, so a bad pack never reaches production. Restart to pick up pack changes.

## Routes

| Route | |
| --- | --- |
| `/` | Today's puzzle. Packs cycle in order, one per UTC day. |
| `/p/{id}` | A puzzle by id. |
| `/p/{id}/files/{path}` | Raw file from the pack. Looked up by exact match against the loaded file list, never joined onto a filesystem path. |
| `/archive` | All puzzles. |
| `/format` | Renders `docs/pack-format.md`. |
| `POST /api/p/{id}/answer` | `{"answer": "..."}` returns `{"correct": bool, "explainer_html"?: string}`. The explainer is only included for a correct answer. |
| `/healthz` | `ok`. |

## Player state

The server stores nothing about players. `app.js` keeps solves, hints used, active time and the streak in `localStorage` under `necropsy:v1`. A streak counts consecutive UTC days with at least one solve and shows as zero after a missed day. Time only counts while the tab is visible.

A returning player's solved answer is kept locally and re-submitted on page load to bring the explainer back, so the server never needs to remember who solved what.

## Security notes

- Pack markdown is rendered with raw HTML converted to text. Highlighted code comes from syntect and is escaped. Everything else goes through maud, which escapes.
- The CSP allows only same-origin scripts, plus the Google Fonts stylesheet and font files. There is no inline script or style, so the theme bootstrap lives in `static/theme.js`.
- The answer endpoint caps the request body at 2 KiB and ignores answers over 512 bytes.
- Answer hashes and explainers are public in the repository. See the note in `pack-format.md`.

## Design

`static/app.css` uses the same tokens and fonts as urashyl.dev (Schibsted Grotesk and JetBrains Mono, the green palette, light and dark). Token colors for code are `--hl-*` and are defined for both themes.

## Not built yet

The CLI, packs 4 and 5 (multi-file, CLI-first), and deployment.
