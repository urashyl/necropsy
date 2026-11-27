A live puzzle gives you a server of your own: a rebuilt C2. Like a real one, it does not help strangers. It answers the implant, and only the implant. Anything else gets a 404, an empty page, or a **decoy**: a convincing but useless answer, like a CDN error page or a parked domain.

So the job is to **be the implant**: send exactly what it sends, in the order it sends it.

## Where the knock comes from

- **A capture** of the implant talking: the method, path, headers and body of each request, and what came back.
- **The sample's strings**: the user agent, paths, header names, format strings like `%s|%s|%s` that say how the body is built.
- **The sample's code**: how it encodes or encrypts the body, and how it reads the reply.

## What servers check

| Check | Example | If wrong |
| --- | --- | --- |
| Path and method | `POST /cdn/v3/collect` | 404, 405 |
| User-Agent | `Updater/5.1 (x64; channel=stable)` | 404 or decoy |
| Custom headers | `X-Session:` 16 hex digits | 404 |
| Body format | base64 of `host|user|os` | 400, or decoy |
| Order | tasking only after a check-in | 404 |
| Cookies or tokens | a value from the check-in reply | 403 |

The check is often a pattern, not an exact value: the server checks that `X-Session` is 16 hex digits, not which ones. Your own made-up machine name works if it has the right shape.

## Following the server

Replies tell the implant what to do next, often encoded:

```json
{"ok":1,"next":"2f63646e2f76332f6a2f3963316538326164","wait":120}
```

`next` is hex: `/cdn/v3/j/9c1e82ad`. A capture shows the path the sandbox got. Your instance gives its own, so read the reply you got, not the one in the capture.

## Work method

1. Write down the implant's request from the capture, every header.
2. Send it as is. If it works, change one thing at a time to learn what the server checks.
3. If it does not, compare your request with the capture's byte by byte: the console's **Request sent** or `curl -v` shows yours.
4. Decode every reply field that looks encoded, and follow it.
