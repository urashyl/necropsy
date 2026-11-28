// The browser solver's script for this pack (docs/browser-solver.md): the
// parser package added to package.json under a scope that is not the real one.
export default async function solve(t) {
  const out = await t.python(`import re
from necropsy import file

diff = (await file("pr-1482.diff")).decode()
added = set(re.findall(r'^\\+\\s+"(@[^"/]+/parser)":', diff, re.M))
fake = [p for p in added if not p.startswith("@typescript-eslint/")]
assert len(fake) == 1, added
print(fake[0])`);
  return out.trim().split("\n").pop();
}
