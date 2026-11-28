// The browser solver's script for this pack (docs/browser-solver.md): the
// user who asked for RC4 tickets to the most service accounts.
export default async function solve(t) {
  const out = await t.python(`import csv, io
from collections import defaultdict
from necropsy import file

rc4 = defaultdict(set)
for r in csv.DictReader(io.StringIO((await file("4769.csv")).decode())):
    if r["TicketEncryptionType"] == "0x17":
        rc4[r["TargetUserName"]].add(r["ServiceName"])
user = max(rc4, key=lambda u: len(rc4[u]))
print(user.split("@")[0])`);
  return out.trim().split("\n").pop();
}
