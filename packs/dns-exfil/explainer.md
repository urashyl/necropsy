The answer is the `access_key_id` in the file the backup server sent, an S3 config. Each player's capture carries a different key.

Two sets of queries look random. The `TXT` lookups under `rep.av-vendor.invalid` come from 10.20.4.40 every minute or two, and their labels are 40 hex digits split in two. That's a SHA-1, which is how antivirus products ask the vendor about a file's reputation. It decodes to bytes, not text.

The other set comes from the backup server, 10.20.8.5, starting at 02:13. Each name looks like `03.mf2xi2bahu….q7f2.t.cdn-metrics.invalid`: a two-digit hex sequence number, a chunk of data, a session ID, and a domain whose name server the attacker runs. The resolver forwards every query to that server, so data gets out even though the backup server can't connect anywhere directly.

The data labels only use `a`–`z` and `2`–`7`. That's base32, because DNS names aren't case-sensitive and base64 is. Sort by sequence number, drop the repeats (the client resent three chunks), add the `=` padding back and decode:

```bash
grep -o '[0-9a-f]\{2\}\.[a-z2-7]*\.q7f2' queries.log | sort -u | grep -v '^ff' \
  | cut -d. -f2 | tr -d '\n' | tr a-z A-Z | python3 -c 'import sys,base64; s=sys.stdin.read(); print(base64.b32decode(s + "=" * (-len(s) % 8)).decode())'
```

That works here because every chunk except the last is 30 bytes, which encodes to 48 base32 characters with no padding, so the chunks can be joined before decoding. The `ff` query at the end is the sender saying it sent 9 chunks.

It's an rclone config, with the credentials for the company's off-site backup bucket. Whoever has it can read, or delete, the backups. That's worth knowing before a ransomware gang tells you.

Blocking direct connections and leaving DNS open is common, and this is the way around it. Watch for a single client making many unique lookups under one domain, especially `TXT` lookups with long labels.
