Install the requests library: `python3 -m pip install requests` (Debian or Ubuntu: `sudo apt install python3-requests`).

Sign in, open the practice puzzle and press **Spin up instance**. Save this as `walk.py`, with your instance's URL:

```python
import requests

BASE = "https://<your instance>"

start = requests.get(BASE + "/api/export").json()
print(start)
key = start["auth"].split("X-Api-Key: ")[1].split()[0]

path = start["first"]
while path:
    r = requests.get(BASE + path, headers={"X-Api-Key": key})
    if r.status_code != 200:
        print(r.status_code, r.text)
        break
    page = r.json()
    print(page["page"], page.get("records"))
    path = page["next"]
print(page.get("export_id"))
```

Run it: `python3 walk.py`.

The first step by hand with curl, to see what the script parses:

```bash
curl -s https://<your instance>/api/export
curl -s -H 'X-Api-Key: <key>' https://<your instance>/api/export/<first page id>
```
