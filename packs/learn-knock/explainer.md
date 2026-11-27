Three requests, each teaching one part of curl:

```bash
curl -i 'https://<instance>/notes?id=3'
curl -i -X POST 'https://<instance>/panel/login'          # 403: no header
curl -i -X POST -H 'X-Operator: nightjar' \
     --data 'user=op&pin=<pin from the note>' 'https://<instance>/panel/login'
```

- **Query parameters** go in the URL after `?`, as `name=value`, joined with `&`. Quote the URL, or the shell treats `&` as "run in the background".
- **-X** sets the method. `--data` alone already makes curl send a POST, so `-X POST` is only needed when there is no body.
- **-H** adds a header. Servers in these puzzles check headers like `User-Agent` and custom ones like `X-Operator`.
- **--data** sends a body and sets `Content-Type: application/x-www-form-urlencoded`, the format of an HTML form.
- **-i** shows the status line and headers. The status says what went wrong: 400 bad request, 401 wrong credentials, 403 not allowed, 405 wrong method.

### What a defender takes from it

Phishing kits and C2 panels often check a header or a parameter before showing anything. Replaying the request exactly as the operator sends it is how an analyst sees what the operator sees.
