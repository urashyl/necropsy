On the terminal path you have used these tools since module 1. Check that your setup has everything the harder puzzles use, with the commands at the end of the concept above, then make sure each of these works on your system.

## A capture, end to end

```bash
tshark -r capture.pcap -Y http.request -T fields -e http.host -e http.request.uri
tshark -r capture.pcap -q -z conv,tcp                  # every TCP conversation
tshark -r capture.pcap -q -z follow,http,ascii,0       # the first one, as text
tshark -r capture.pcap --export-objects http,objects/  # every file sent over HTTP
```

The last one saves each HTTP response body as a file: the quickest way to pull a payload out of a capture. Do not open what it saves; read it with `xxd` and `strings`.

## A request, three ways

With the **Knock knock** instance running:

```bash
curl -v -H 'X-Operator: nightjar' --data 'user=op&pin=<pin>' https://<instance>/panel/login
```

```python
import requests
r = requests.post("https://<instance>/panel/login",
                  headers={"X-Operator": "nightjar"},
                  data={"user": "op", "pin": "<pin>"})
print(r.status_code, r.text)
```

and once more in the workspace's console, which shows the raw bytes of both directions. The three should get the same answer.

When all of this works, you have everything the tier 4 and 5 puzzles need.
