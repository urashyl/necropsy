1. Open the practice puzzle's workspace. In **Pcap**, filter `http.request` and **Follow HTTP stream** on the POST. That is the check-in, and the server's reply.
2. In **Files**, read `strings.txt`: the user agent, the header name, `%016llx` (16 hex digits) and `%s|%s|%s` (three fields joined by `|`).
3. Spin up the instance. In the **Console**, first send a plain `curl -i https://<instance>/cdn/v3/collect`: the CDN's 404. That is the decoy.
4. Build the check-in. Make your own body: in the **Decoder**, `To Base64` on `PC-01|you|Windows 10`. Then:

```bash
curl -i -X POST -H 'User-Agent: Updater/5.1 (x64; channel=stable)' \
     -H 'X-Session: 0123456789abcdef' -H 'Content-Type: text/plain' \
     --data '<your base64>' https://<instance>/cdn/v3/collect
```

5. The reply's `next` is hex. **From Hex** in the decoder gives a path.
6. `GET` that path with the same User-Agent. Leave out the User-Agent once to see the decoy again.
7. The key is base64, reversed: in the decoder, `Reverse` then `From Base64`.

The same in the **Notebook**, which keeps values between steps:

```python
from necropsy import request
import base64
ua = {"User-Agent": "Updater/5.1 (x64; channel=stable)"}
body = base64.b64encode(b"PC-01|you|Windows 10").decode()
r = await request("POST", "/cdn/v3/collect", headers={**ua, "X-Session": "0123456789abcdef"}, data=body)
path = bytes.fromhex(r.json()["next"]).decode()
t = (await request("GET", path, headers=ua)).json()
print(base64.b64decode(t["v"][::-1]).decode())
```
