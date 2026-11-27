Download `capture.pcap` and `strings.txt` from the puzzle page, and spin up the instance.

1. Read the check-in:

```bash
tshark -r capture.pcap -Y http.request -T fields -e http.request.method -e http.request.uri -e http.user_agent
tshark -r capture.pcap -q -z follow,http,ascii,1
```

2. Send the decoy request, then the real one:

```bash
I=https://<your instance>
UA='Updater/5.1 (x64; channel=stable)'
curl -i "$I/cdn/v3/collect"
curl -i -A "$UA" "$I/cdn/v3/collect"          # still 404: wrong method, no session, no body
BODY=$(printf 'PC-01|you|Windows 10' | base64)
curl -i -X POST -H "User-Agent: $UA" -H 'X-Session: 0123456789abcdef' -H 'Content-Type: text/plain' \
     --data "$BODY" "$I/cdn/v3/collect"
```

3. Decode `next` and fetch the task:

```bash
NEXT=$(echo '<next value>' | xxd -r -p)
curl -s -H "User-Agent: $UA" "$I$NEXT"
```

4. Decode the key: `echo '<v value>' | rev | base64 -d`.

Or all of it in one Python script with `requests`, as in module 7: a `Session` keeps cookies between steps if a server sets any.
