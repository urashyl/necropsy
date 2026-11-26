# Live puzzles

A live puzzle is a server. A signed-in player presses **Spin up instance** and gets a link like `https://k3x9q2mfa7.necropsy.urashyl.dev`. That link is a small simulated server, described by the pack, that lives for a while and answers whatever the player sends it. They attack it, emulate a client of it, or pick through what it exposes, and the flag is what it hands over.

There are two ways to back a live puzzle:

- **Simulated** (this page): a `live.toml` of routes. A response is a status, headers and a body taken from the pack. Nothing is executed: there is no shell behind a "webshell", no outbound request from a handler, and no access to the machine's files. It is cheap and safe, good for protocol emulation and exposed-file puzzles.
- **Real** ([range.md](range.md)): a sandboxed container per player running real software, such as a working webshell or a vulnerable app, on a VM of its own (the range). A `live.toml` with `[container] image = "..."`. It needs the range VM and costs memory there.

Everything else here (instances, links, flags, the captcha, the proxy setup) applies to both.

## How it works

- `*.<NECROPSY_LIVE_DOMAIN>` is pointed at the same process as the site. Requests whose `Host` falls under that domain are answered from the scenario; anything else is the normal site. The two never share pages, cookies or CSRF logic.
- An instance has a random label (the subdomain): four diceware words such as `plank-orbit-velvet-cargo`, about 51 bits, a time to live (`ttl_minutes`, at most 240), and a random seed kept in the database. The seed makes the flag, and every `{{d:name}}` value, different for every instance. Flags cannot be passed between players.
- The player submits the flag on the puzzle page as usual. It is checked against the flags of that player's own instances of the pack, including recently expired ones, and the solve is recorded like any other.
- Limits: one running instance per pack per player, three running in all, 100 requests per 10 seconds per instance, request bodies up to 1 MiB. The latest 100 requests are shown to the player on the puzzle page.

## Setting it up

Set `NECROPSY_LIVE_DOMAIN` (see `.env.example`). Without it live puzzles are hidden everywhere.

Your reverse proxy needs three things:

1. A DNS record and a certificate for `*.necropsy.urashyl.dev`. A certificate for `*.urashyl.dev` does **not** cover it, since a wildcard covers one label. Issue a separate wildcard (DNS challenge).
2. A block that sends the wildcard to this server and leaves `Host` as it is (Caddy does by default):

```caddyfile
@necropsy_live host *.necropsy.urashyl.dev
handle @necropsy_live {
	request_body {
		max_size 1MB
	}
	reverse_proxy http://192.168.100.22:8080 {
		header_up X-Real-IP {remote_host}
		transport http {
			read_timeout 60s
			write_timeout 60s
		}
	}
}
```

3. When you move to `necropsy.dev`, change `NECROPSY_LIVE_DOMAIN` and add the same block for `*.necropsy.dev`. Pack content does not change, since hosts in responses use `{{host}}`.

If your proxy cannot pass `Host` through, set `NECROPSY_LIVE_HOST_HEADER` to a header holding the original host.

With the bundled blue-green Caddy, nothing changes: it answers on any host and passes `Host` through. Its 16 KB request body cap applies to instances too.

### Why the site's session cookie is `__Host-`

An instance is a subdomain of the site, and its pages may run scripts (a cookie logger sample, say). A page on a subdomain is allowed to set cookies for the parent domain, so it could plant its own `necropsy_session` and get a victim signed in to the attacker's account. Over https the session cookie is therefore named `__Host-necropsy_session`: browsers accept that name only when it is set by the exact host, with `Secure`, `Path=/` and no `Domain`. Signing in again after an upgrade is a one-off.

Starting an instance asks for a captcha (see [accounts.md](accounts.md#captcha)). Cross-site POSTs are rejected for the main site, and an instance's origin is not the site's origin, so a page on an instance cannot post to the site's API on a player's behalf.

## The scenario file

`live.toml` sits next to `pack.toml`. The pack's `modes` includes `live`, there is no `answer` file, and `files/` may be empty.

```toml
ttl_minutes = 60            # lifetime of an instance
flag_prefix = "pebble"      # the flag is pebble{...}, unique per instance

[[route]]                   # routes are tried in order, the first match wins
method = "POST"             # optional; omit for any method (GET also matches HEAD)
path = "/api/v2/sync"       # exact, or a trailing * for a prefix
needs = ["checked_in"]      # optional: instance state flags that must be set
sets = ["got_token"]        # optional: flags to set when this route fires
[route.when]                # every condition must hold
header = { "user-agent" = 're:^Pebble/2\.4' }
query  = { id = "*" }
cookie = { sid = "{{d:sid}}" }
form   = { d = "re:^[A-Za-z0-9+/]{20,}={0,2}$" }
body   = "re:token"
basic_auth = "admin:{{d:panelpw}}"
[route.respond]
status = 200
content_type = "application/json"
headers = { "x-whatever" = "value" }
body = '{"flag":"{{flag}}"}'     # inline, templated
# file = "sample.js"             # or a file from live/, sent as is and not templated

[fallback]                  # for requests no route matched; default is a plain 404
status = 404
body = "Not Found"
```

A condition value is matched exactly, as a regular expression when it starts with `re:`, or for presence alone when it is `*`. A missing header, cookie or field never matches.

Templates, available in paths, condition values and inline bodies:

| | |
| --- | --- |
| `{{flag}}` | The instance's flag. |
| `{{instance}}` | The instance label. |
| `{{host}}` | The host the request arrived on. |
| `{{d:name}}` | 12 hex digits derived from the instance's seed and `name`. Different for each player, stable within an instance. Use for passwords, tokens and paths a player must discover. |
| `{{b64:X}}` `{{hex:X}}` `{{rev:X}}` | Any of the above, base64-encoded, as hex, or reversed. They nest, innermost first: `{{rev:b64:d:key}}`. For a sample that hides a per-player value, such as a C2 path the player has to decode. |

State flags (`needs` and `sets`) let a scenario have steps: one route sets `checked_in`, and another only answers once it is set. State is stored with the instance.

`pack-tool validate packs` checks the scenario: patterns compile, files exist, statuses are sane.

### Samples in a live pack

Real samples are fine. A `file` response is sent as is, so give an executable the right `content_type` and a `content-disposition = "attachment"` header, and a harmless name, so a browser offers to save it rather than running it. Scripts that would run in a page (a cookie logger, say) work in the player's browser as they would anywhere, scoped to that instance's own origin.

## The malware analysis library

The always-available `live` track is a library that grows by adding a directory. Each puzzle in it is built the same way: the instance serves a **sample** (the thing to analyse), and also plays the **operator's server** the sample talks to. The player has to read the sample, learn how it hides its server's address and how it authenticates, and then do what the sample would do against their own instance.

| # | Puzzle | The sample | What it teaches |
| --- | --- | --- | --- |
| 1 | `cookie-logger` | Stored-XSS cookie stealer, JavaScript | Reading a planted script; `atob`; an exposed collector index |
| 2 | `skimmer` | Magecart-style card skimmer, obfuscated JavaScript | String arrays, reversed base64, hex; the leftover debug function |
| 3 | `ps-loader` | `.bat` with `powershell -enc`, then two PowerShell stages | `-enc` is UTF-16LE base64; User-Agent gating; staged delivery |
| 4 | `vbs-dropper` | VBScript with character-shifted strings | Caesar-shift decoding; `StrReverse` tokens; `Execute` of the server's reply |
| 5 | `macro-stager` | Extracted VBA macro | Defanged URLs, hex paths, sandbox checks, header fingerprinting |
| 6 | `xor-implant` | Python implant with XOR'd strings | Single-byte XOR; being the other side of a C2 protocol |
| 7 | `dga-sinkhole` | Domain generation algorithm | Running a DGA for a chosen date; sinkholes |
| 8 | `ransom-recovery` | Ransomware encryptor, Python | Key derivation flaws; repeating-key XOR; what recovery needs |

Conventions for new ones:

- Samples are served as `.txt` attachments (`content-disposition: attachment`, a name like `invoice.bat.txt`), so a click cannot run one. The text is real; the player decides what to run and where.
- A sample's server is a name under `.invalid`, which cannot resolve. The brief says so and tells the player to send the sample's requests to their instance link instead.
- Put what the player must decode in the sample, per player, with the encoding templates (`{{rev:b64:d:gate}}`). Put what the server checks in `when` conditions and `needs`/`sets` state, so skipping a step is a 404, not a shortcut.
- Give every puzzle a negative test (wrong header, wrong order, wrong key) as well as a walkthrough: see the `library` module in `crates/necropsy-web/src/tests.rs`.
