// The browser solver's script for this pack (docs/browser-solver.md): join the
// base32 chunks of the DNS tunnel in sequence order, decode, read the key.
export default async function solve(t) {
  const out = await t.python(`import base64, re
from collections import defaultdict
from necropsy import file

chunks = defaultdict(dict)
for name in re.findall(r"\\(([^)]+)\\): query:", (await file("queries.log")).decode()):
    m = re.fullmatch(r"([0-9a-f]{2})\\.([a-z2-7]{8,})\\.(.+)", name)
    if m and m.group(1) != "ff":
        chunks[m.group(3)][int(m.group(1), 16)] = m.group(2)
tunnel = max(chunks.values(), key=len)
s = "".join(tunnel[i] for i in sorted(tunnel)).upper()
text = base64.b32decode(s + "=" * (-len(s) % 8)).decode()
print(re.search(r"access_key_id\\s*=\\s*(\\S+)", text).group(1))`);
  return out.trim().split("\n").pop();
}
