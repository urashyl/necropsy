# The in-browser toolkit

Every puzzle has a workspace at `/w/<id>` (the puzzle page links to it as **Open the workspace**). It holds the tools a difficulty 1 to 3 puzzle needs, running in the player's browser, so nothing has to be installed:

| Tab | Tool | Terminal equivalent shown |
| --- | --- | --- |
| Files | The puzzle's files, with buttons to open each in another tool | `curl -O <file URL>` |
| Pcap | Wireshark's dissectors compiled to WebAssembly ([Wiregasm](https://github.com/good-tools/wiregasm)): packet list, display filters, protocol tree, Follow TCP and HTTP stream | `tshark -r ... -Y '<filter>'`, `tshark -q -z follow,...` |
| Decoder | [CyberChef](https://github.com/gchq/CyberChef), served by the site | `base64 -d`, `xxd -r -p`, Python |
| Console | A request console that sends curl commands to the player's live instance (live puzzles only) | the curl command itself |
| Notebook | Python 3 ([Pyodide](https://pyodide.org)), with a `necropsy` helper module | `python3 script.py` with `requests` |
| Hex | Hex dump and strings, with offsets | `xxd`, `strings -t x -n N` |

There is no virtual machine and no shell on the server: everything but the console runs in the browser, and the console reaches only the player's own instance. Answers are still checked by the server.

The page also has the answer form, hints and clock, the same as the puzzle page (they are driven by `app.js`).

## Installing the toolkit

The three third-party builds are too large for the repository (about 160 MB unpacked). `toolkit/fetch.sh` downloads the versions pinned in `toolkit/sources.txt`, checks each download's SHA-256, and unpacks it into `toolkit/dist/<name>-<version>/`, with a `.gz` copy of every file over 1 KiB:

```bash
toolkit/fetch.sh               # into toolkit/dist (gitignored)
toolkit/fetch.sh /some/dir     # anywhere else
```

It needs `curl`, `unzip` and `gzip`, and network access to `registry.npmjs.org`, `github.com` and `release-assets.githubusercontent.com`. Running it again skips versions already there.

- **Docker:** the image build runs it in a stage of its own and copies the result to `/app/toolkit` (see [docker.md](docker.md)).
- **Installed service:** `deploy/install.sh` runs it into `~/.local/share/necropsy/toolkit` (see [deploy.md](deploy.md)).
- **Local development:** run it once; `cargo run -p necropsy-web` finds `toolkit/dist`.

| Setting | Default | |
| --- | --- | --- |
| `NECROPSY_TOOLKIT` | `toolkit/dist` | Directory with the `<name>-<version>` directories. |

The server lists the directory at startup and logs how many files it found. A tool that is missing is not an error: the site runs, and that tool's tab says it is not installed on this server. The log warns at startup.

### Updating a tool

1. Change the version, SHA-256 and URL in `toolkit/sources.txt`. Get the SHA-256 by downloading the file and running `sha256sum` on it.
2. Run `toolkit/fetch.sh` and check the workspace in a browser: open a capture, run a CyberChef recipe, run a Python cell.
3. Check the pane scripts still match the tool's API (`static/toolkit/pcap-worker.js` uses Wiregasm's low-level `DissectSession`, `py-worker.js` uses `loadPyodide`).

Directory names include the version, so URLs change with it and the browser fetches the new files. CyberChef is taken from its GitHub release zip, not npm: the npm package is the library, without the web app.

## How the files are served

- `/toolkit/<name>-<version>/<path>` serves a file found at startup, looked up in a list, never by joining the request onto a path. Large files are streamed. They are sent with `Cache-Control: public, max-age=31536000, immutable`, and the `.gz` copy with `Content-Encoding: gzip` to a browser that accepts gzip (Wireshark's 73 MB WebAssembly is 20 MB compressed).
- `/static/toolkit/<file>` serves the workspace's own scripts, which are compiled into the binary. They are revalidated by ETag on every load (`Cache-Control: no-cache`), so a module and the modules it imports always come from the same release.
- The browser keeps the large files in its HTTP cache, so a player downloads each tool once per version.

## Security

- **The page** keeps the site's CSP, plus `frame-src 'self'` (for the decoder) and `worker-src 'self'`. It never allows `eval`.
- **The workers.** Wireshark and Python run in Web Workers. A worker's script is served with its own CSP: `script-src 'self' 'unsafe-eval' 'wasm-unsafe-eval'; connect-src 'self'`. `wasm-unsafe-eval` is needed to compile WebAssembly; `unsafe-eval` because Wiregasm's bindings are built with `new Function`. A worker has no DOM and loads nothing but the site's own files.
- **CyberChef** needs inline scripts and `eval`, so it never runs in the site's origin. Its files are served with `Content-Security-Policy: sandbox allow-scripts ...` without `allow-same-origin`, and the frame has the same `sandbox` attribute. It runs in an opaque origin: it cannot read the site's cookies or storage, and requests it makes are cross-site, so the session cookie (`SameSite=Lax`) is not sent. Its files carry `Access-Control-Allow-Origin: *` so the opaque origin can load its own fonts; they are public files.
- **Other toolkit files** are sent with `Content-Security-Policy: default-src 'none'; sandbox`, in case one is opened directly.
- Pcap files are parsed by Wireshark's dissectors in the player's own browser, never on the server.

## Request console

The console turns a curl command into the raw HTTP request curl would send, sends the bytes over a WebSocket to the player's instance, and shows the raw response. A browser's `fetch` cannot set `User-Agent`, `Host`, `Cookie` and other forbidden headers; C2 puzzles depend on them.

### Supported curl

| | |
| --- | --- |
| URL | `http://` or `https://`, with path and query. Its host must be the player's instance. One per command. |
| `-X`, `--request` | Method. Without it, `GET`, or `POST` when there is a body. |
| `-H`, `--header` | Repeatable. Replaces a header curl would send (`Host`, `User-Agent: curl/8.5.0`, `Accept: */*`). `-H 'Name:'` removes one. |
| `-d`, `--data`, `--data-raw`, `--data-ascii` | Repeatable, joined with `&`. Adds `Content-Type: application/x-www-form-urlencoded` unless a `-H` sets one, and `Content-Length`. `@file` is refused (except with `--data-raw`, where `@` is literal). |
| `-i`, `--include`; `-v`, `--verbose` | What **Terminal output** shows, as curl would: the body, headers too, or the exchange with `>` and `<`. **Request sent** and **Response received** always show every byte. |

Quoting follows the POSIX shell: single quotes, double quotes with `\"`, backslash escapes, and `\` at the end of a line. Any other flag is an error that names it, often with what to use instead (`-A`: use `-H 'User-Agent: ...'`). The parser is `static/toolkit/curl.js`, tested by `crates/necropsy-web/tests-js/curl.test.mjs`.

### Block mode

**Block mode** shows the command as `curl <instance>` and a path box, with buttons to add a method, a header, a body or a query parameter; show headers and verbose are checkboxes. The command, as text, is under the blocks and changes as they change. Unticking it keeps the command as text. Ticking it converts the text back to blocks when it uses only what blocks can show and its URL is the instance; otherwise it says why and stays in text.

### Transport

`GET /api/live/{label}/ws` opens the WebSocket. It is refused unless:

- `Origin` is exactly the site's `NECROPSY_BASE_URL` (403). Browsers send `Origin` on every WebSocket; a missing one is refused.
- The player is signed in (401).
- Live puzzles are on, and `{label}` is an instance of this player that has not expired (404).

Each message is one raw HTTP/1.1 request, at most 1 MiB plus 64 KiB for headers. The server parses it, reduces the request target to its path and query, sets `Host` to `<label>.<NECROPSY_LIVE_DOMAIN>`, replaces the `NECROPSY_CLIENT_IP_HEADER` header (if set) with the player's real address, and passes it to the same handler that answers the instance's subdomain. So scenario matching, container proxying, the instance's rate limit (100 requests per 10 seconds), its body limit and its request log are all the same. The reply is a binary message with the raw response (status line, headers, a `content-length` when the handler set none, body up to 8 MB), or a text message `{"error": "..."}` saying why the request could not be sent: no blank line after the headers, a bad header, `Content-Length` longer than the body, `Transfer-Encoding`, or an instance that has stopped since the connection opened. A connection with no message for 15 minutes is closed.

It is not a general proxy: the target is fixed by the label, which must be the player's own instance, and nothing in the request can change it.

### Reverse proxy

The WebSocket goes to the main site's host, so the main site's proxy block must pass WebSocket upgrades. Caddy does by default. nginx needs:

```nginx
location /api/live/ {
    proxy_pass http://127.0.0.1:8080;
    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
    proxy_set_header Host $host;
    proxy_read_timeout 900s;
}
```

## Notebook

Python 3 with its standard library, in a worker. Cells run with `await` allowed at the top level; **Shift+Enter** runs one. Cells are kept in the browser's localStorage per puzzle. Third-party packages are not installed: Pyodide's other packages are not downloaded, so `numpy` or `cryptography` cannot be imported. A puzzle that needs them is difficulty 4 or more.

The `necropsy` module (`static/toolkit/necropsy.py`):

| | |
| --- | --- |
| `await file(name)` | A file of this puzzle, as bytes. |
| `await request(method, url, headers=None, data=None, json=None, params=None)` | Sends a request to the player's instance over its own WebSocket, the same transport as the console, and returns a `Response` with `status`, `reason`, `headers` (lower-case names), `content`, `text` and `json()`. `url` is a path or a URL on the instance. Its default `User-Agent` is `python-requests/2.32.3`, as the real library sends. |
| `build(...)` | The raw request `request()` would send, as bytes. |

On the player's own computer the same code is `requests.request(...)` and `open(name, "rb").read()`.

## Hints that open the decoder

A hint can end with `{decoder: <CyberChef recipe>}` (see [pack-format.md](pack-format.md#hints)). The puzzle page shows the hint text and an **Open in the decoder** link to `/w/<id>#decoder=<recipe>`, which opens the workspace with that recipe in the decoder. The workspace also reads `#tool=<tab>` to open a tab.

## When something breaks

| Symptom | Cause, and what to do |
| --- | --- |
| A tab says the tool is not installed | `NECROPSY_TOOLKIT` does not point at a directory with that tool. Check the startup log, run `toolkit/fetch.sh` into the right place, restart. |
| `fetch.sh` stops with a SHA-256 mismatch | The download changed or is incomplete. Do not edit the sum to match; download again, and if it still differs, check the upstream release before trusting it. |
| The pcap viewer or the notebook never finishes loading | Open the browser console. A CSP error naming `eval` or `wasm` means a proxy in front of the site replaced the worker's `Content-Security-Policy` header: let the application's header through. A 404 on `/toolkit/...` means the version in the page differs from the files on disk: restart after fetching. |
| The decoder frame is blank | A proxy added `X-Frame-Options: DENY` or its own CSP to `/toolkit/cyberchef-*`; let the application's headers through. |
| The console says it could not connect | The WebSocket upgrade did not reach the site (see Reverse proxy above), or `NECROPSY_BASE_URL` does not match the address in the browser, so `Origin` is refused. |
| The console answers 404 for a running instance | The instance belongs to another account, or expired. Start it again. |
