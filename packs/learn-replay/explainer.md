The export has 30 pages. Each answers only with the API key, and only after the page that links to it, so the walk has to be done in order.

In the notebook:

```python
from necropsy import request

start = (await request("GET", "/api/export")).json()
key = start["auth"].split("X-Api-Key: ")[1].split()[0]
path = start["first"]
while path:
    page = (await request("GET", path, headers={"X-Api-Key": key})).json()
    print(page["page"], page.get("records"))
    path = page["next"]
print(page["export_id"])
```

On your own computer, the same loop with `requests.get(base + path, headers=...)`, or in a shell:

```bash
next=$(curl -s "$BASE/api/export" | python3 -c 'import json,sys; print(json.load(sys.stdin)["first"])')
```

and a `while` loop around `curl -s -H "X-Api-Key: $KEY"`.

### What a defender takes from it

Anything done more than three times by hand is worth a script: it is faster, and it records exactly what was sent. The same loop walks a real C2's task queue or a stolen-data index during an investigation.
