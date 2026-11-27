By hand, one request is quick. Thirty requests in order, each using a value from the last, or a thousand strings to decode, is slow and error-prone. A short script does it in seconds, and keeps an exact record of what was sent.

## When to script

- **Chains.** Each response says where to go next (a path, a token, a page number).
- **Repetition.** The same request with one value changed: IDs 1 to 500, every task in a queue.
- **Bulk decoding.** Every string in a table, every message in a capture.
- **Anything you have done three times by hand.**

## Python and requests

`requests` is the usual Python library for HTTP. The pieces map one-to-one to curl:

```python
import requests

r = requests.post(
    "https://c2.example.test/gate",           # the URL
    headers={"User-Agent": "Pebble/2.4"},     # -H
    data={"id": "7", "st": "idle"},           # --data, form-encoded
    params={"v": "2"},                        # ?v=2 in the URL
)
print(r.status_code)     # 200
print(r.headers)         # response headers
print(r.text)            # body as text
print(r.json())          # body parsed as JSON
```

`json=` instead of `data=` sends a JSON body. `r.content` is the body as bytes.

## A chain

```python
path = "/api/export"
while path:
    r = requests.get(BASE + path, headers={"X-Api-Key": key})
    page = r.json()
    print(page["page"])
    path = page["next"]        # None on the last page ends the loop
```

## curl in a loop

For simple repetition a shell loop is enough:

```bash
for id in $(seq 1 50); do
  curl -s "https://c2.example.test/task?id=$id"; echo
done
```

`-s` hides the progress meter. When each request needs a value from the last response, switch to Python: parsing JSON in shell is possible (`jq`) but gets awkward fast.

## Good habits

- Print as you go, so you see where it stops.
- Stop on an error status instead of carrying on: `if r.status_code != 200: print(r.text); break`.
- Do not hammer a server: a live instance here answers 100 requests per 10 seconds. A real C2 may notice you.
