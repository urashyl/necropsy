The answer is `sync.pagepilot-cdn.invalid`.

Three hosts appear in the script, and two of them look like the same vendor. The `pagepilot-assets` hosts only fetch a config file and report a version number. The odd one out is `pagepilot-cdn`, a different second-level domain, and it is the only host that receives anything sensitive.

What to look at when you read an extension like this:

- **The data flow, not the hostnames.** Find where `chrome.cookies.getAll` is called and follow its result to the network call.
- **`sendBeacon`.** It is built for fire-and-forget analytics, which makes it a convenient way to exfiltrate data without blocking the page or surfacing errors.
- **The trigger.** `webRequest.onCompleted` with `<all_urls>` and a hostname filter means it only acts on pages worth stealing from, which keeps its traffic low.
- **Permissions.** An extension that needs `cookies`, `webRequest` and `<all_urls>` together can do all of this. A tab helper has no reason to ask for them.

The base64 step is not encryption. Anyone who can see the request can decode the cookie list.
