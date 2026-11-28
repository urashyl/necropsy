// The browser solver's script for this pack (docs/browser-solver.md): the
// account with a successful legacy-authentication (BAV2ROPC) sign-in.
export default async function solve(t) {
  const out = await t.python(`import json
from necropsy import file

hits = set()
for line in (await file("signins.jsonl")).decode().splitlines():
    if not line.strip():
        continue
    e = json.loads(line)
    if e["userAgent"] == "BAV2ROPC" and e["status"]["errorCode"] == 0:
        hits.add(e["userPrincipalName"])
assert len(hits) == 1, hits
print(hits.pop())`);
  return out.trim().split("\n").pop();
}
