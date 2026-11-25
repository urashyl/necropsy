The answer is `https://pkg.ansi-tint-mirror.invalid/v2/linux-x64/helper`.

`index.js` is the real package and does what it says. The new file is `scripts/postinstall.js`, which `package.json` runs on every install, before anyone imports anything. It XORs a base64 string with a repeating key, and the key is `pkg.name`, the package's own name: `ansi-tint`. You can't decode it from that one file, which is the idea.

```bash
python3 - <<'PY'
import base64, re
src = open("scripts/postinstall.js").read()
blob = "".join(re.findall(r'"([A-Za-z0-9+/=]{20,})"', src))
key = b"ansi-tint"
data = base64.b64decode(blob)
print(bytes(b ^ key[i % len(key)] for i, b in enumerate(data)).decode())
PY
```

The decoded code picks a host by platform. Windows and macOS get their own, and anything else falls through to `pkg.ansi-tint-mirror.invalid`. The path is `/v2/`, then `process.platform`, a dash, and `os.arch()`. On an Ubuntu runner those are `linux` and `x64`. Node calls x86-64 `x64`, not `amd64` or `x86_64`.

Two other details are why anyone would bother. It only runs when `CI` or `GITHUB_ACTIONS` is set, so a developer installing it on their laptop never sees anything happen, and it goes off on every CI run. And it sends all of `process.env`, base64-encoded, in an `x-e` header. On a build runner, the environment is where the secrets live: deploy keys, cloud credentials, and the npm token you'd need to publish the next poisoned package. Then it runs whatever the server sends back.

Several real npm compromises in 2025 worked much like this. If it had been your runner, removing the package wouldn't be the fix. Rotate every secret that build could see, then pin your dependencies and use `npm ci --ignore-scripts` wherever the build doesn't need install scripts.
