The answer is `secure-account-verify.invalid`.

Each hop uses a different way of moving the browser along:

1. **`Location` header.** A `302` response with `Location:` is the plain HTTP redirect. The short link sends you to a tracking host.
2. **`<meta http-equiv="refresh">`.** The page loads, and the tag tells the browser to go somewhere else straight away.
3. **JavaScript.** `window.location = ...` after a short delay. The address is base64 inside `atob(...)`, so a search for the host name finds nothing:

```bash
echo 'aHR0cHM6Ly9zZWN1cmUtYWNjb3VudC12ZXJpZnkuaW52YWxpZC9sb2dpbg==' | base64 -d
```

4. **The destination**, a copy of a bank's sign-in form whose form posts to `/collect`.

Why bother with a chain? Every hop is a place to filter. The first hosts can show a harmless page to scanners and reputation services, and only send a real victim (a phone, the right country) on to the last one. Blocking the final host stops that campaign, but the earlier hosts are the ones that appear in your logs when someone clicked, so you want them all.

When you look at one of these yourself, fetch it with a tool that does not follow redirects, so you can read each response:

```bash
curl -sS -i http://short.example.invalid/x7Kp     # a header redirect shows Location:
```
