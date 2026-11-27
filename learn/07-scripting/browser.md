The workspace's **Notebook** runs Python in your browser. Its `necropsy` module has two helpers: `file(name)` returns a puzzle file as bytes, and `request(method, path, headers=..., data=...)` sends a request to the instance through the same connection as the console. Use `await` in front of both.

1. Sign in, open the practice puzzle's workspace and press **Spin up instance**.
2. Open the **Notebook** tab. Python loads once (about 10 MB). The first cell shows the helpers.
3. In a new cell, ask for the start of the export:

```python
from necropsy import request
r = await request("GET", "/api/export")
print(r.status, r.text)
start = r.json()
```

4. The response names the key and the first page. Fetch page 1 by hand, with the header:

```python
key = start["auth"].split("X-Api-Key: ")[1].split()[0]
page = (await request("GET", start["first"], headers={"X-Api-Key": key})).json()
print(page)
```

5. Each page links to the next. Turn it into a loop that stops when `next` is `None`, printing each page number. The last page has `export_id`.
6. **Shift+Enter** runs a cell. Cells are kept in this browser, per puzzle.

The Terminal line shows how this maps to Python on your own computer: `request()` is `requests.request()`, and `file()` is `open(name, "rb").read()`.
