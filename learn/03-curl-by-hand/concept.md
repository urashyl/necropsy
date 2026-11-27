**curl** sends one HTTP request and prints the answer. It is how an analyst talks to a server directly: replay an implant's check-in, poke at a panel, fetch a payload without a browser running it.

## The parts of a request, and the flag for each

```bash
curl -i -X POST \
     -H 'User-Agent: Pebble/2.4' \
     -H 'X-Client-Id: 7f3a9c1e' \
     --data 'id=7&st=idle' \
     'https://c2.example.test/gate?v=2'
```

| Part | Flag | Notes |
| --- | --- | --- |
| URL | the argument | Scheme, host, path, then `?` and the query: `name=value` pairs joined by `&`. |
| Method | `-X POST` | Without it: `GET`, or `POST` when there is a body. |
| Header | `-H 'Name: value'` | Repeat for each header. A `-H` replaces the header curl would send itself, like `User-Agent: curl/8.5.0`. `-H 'Accept:'` removes one. |
| Body | `--data '...'` or `-d` | Sets `Content-Type: application/x-www-form-urlencoded` unless you send your own. Several `--data` are joined with `&`. |
| Show headers | `-i` | The status line and response headers, then the body. |
| Verbose | `-v` | Also the request as sent, lines starting with `>`. |

Quote anything with spaces, `&`, `?`, `;` or `*` in it. Single quotes are safest: the shell changes nothing inside them. A long command can be split with `\` at the end of a line.

## Reading the answer

```http
HTTP/1.1 403 Forbidden
content-type: text/plain

Operators only: the X-Operator header is missing or wrong.
```

The first line is the **status**:

| Code | Means | Usually |
| --- | --- | --- |
| 200 | OK | It worked. |
| 301, 302 | Moved | Go to the `Location` header. curl does not follow it unless asked. |
| 400 | Bad request | A parameter or the body is malformed or missing. |
| 401 | Unauthorized | Credentials missing or wrong. |
| 403 | Forbidden | You are not allowed: often a missing header or token. |
| 404 | Not found | Wrong path, or a server pretending there is nothing here. |
| 405 | Method not allowed | Right path, wrong method. |
| 500 | Server error | You broke something on the other side. |

Then the response headers, a blank line, and the body. A C2 that wants to hide shows a 404 or a decoy page to anything that is not its implant, so an error is information: change one thing at a time and watch what changes.

## A browser cannot do this

A web page can't set `User-Agent`, `Host` or `Cookie` on its own requests, and C2 servers often check those. curl can. The workspace's console sends your curl command as raw HTTP over its own connection to the instance, so those headers go through as written.
