#!/usr/bin/env python3
"""Reference walkthrough and checks for elf-lab (used by test.sh in CI, and locally).
Static analysis of the ELF: find the config blob, decrypt it, then use the gateway.

  solve.py <base url> [--header 'Name: value']... [--flag EXPECTED]
"""
import argparse
import re
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request

ap = argparse.ArgumentParser()
ap.add_argument("base")
ap.add_argument("--header", action="append", default=[])
ap.add_argument("--flag")
args = ap.parse_args()
EXTRA = dict(h.split(": ", 1) for h in args.header)
SRC = open(__file__.rsplit("/", 1)[0] + "/implant.c").read()


def die(m):
    print(f"elf-lab: FAIL: {m}", file=sys.stderr)
    sys.exit(1)


def ok(m):
    print(f"elf-lab: ok: {m}", file=sys.stderr)


def http(method, path, data=None, headers=None, raw=False):
    req = urllib.request.Request(args.base + path, data=data, method=method)
    for k, v in {**EXTRA, **(headers or {})}.items():
        req.add_header(k, v)
    try:
        with urllib.request.urlopen(req, timeout=20) as r:
            b = r.read()
            return r.status, b if raw else b.decode()
    except urllib.error.HTTPError as e:
        return e.code, e.read() if raw else e.read().decode()


def rc4(key, data):
    s = list(range(256)); j = 0
    for i in range(256):
        j = (j + s[i] + key[i % len(key)]) & 255
        s[i], s[j] = s[j], s[i]
    i = j = 0; out = bytearray()
    for b in data:
        i = (i + 1) & 255; j = (j + s[i]) & 255
        s[i], s[j] = s[j], s[i]
        out.append(b ^ s[(s[i] + s[j]) & 255])
    return bytes(out)


status, elf = http("GET", "/samples/svcupd.bin", raw=True)
status == 200 and elf[:4] == b"\x7fELF" or die("no ELF sample")

# What `strings` shows must not give the gateway away.
text = b"\n".join(re.findall(rb"[\x20-\x7e]{4,}", elf)).decode()
for secret in ("gw.cdn-metrics", "vendorsync", "/gw/", "svcupd/"):
    secret in text and die(f"strings reveals {secret!r}")
ok("strings shows neither the gateway, the key nor the config")

# The analyst's route: the key table and the XOR byte are in the program. Read them
# from the source here, then check they really are in the binary.
kenc = bytes(int(x, 16) for x in re.search(r"K_ENC\[12\] = \{([^}]*)\}", SRC).group(1).split(","))
xor = int(re.search(r"K_ENC\[i\] \^ (0x[0-9a-f]+)", SRC).group(1), 16)
kenc in elf or die("the key table is not in the binary")
key = bytes(b ^ xor for b in kenc)
ok("the key table is in the binary; its key is recovered")

marker = bytes([0x4E, 0x43, 0x46, 0x47, 0xA5, 0x5A, 0xC3, 0x3C])
elf.count(marker) == 1 or die("the config marker is not unique")
at = elf.index(marker)
n = int.from_bytes(elf[at + 8 : at + 10], "little")
cfg = rc4(key, elf[at + 10 : at + 10 + n]).decode()
conf = dict(line.split("=", 1) for line in cfg.splitlines())
conf["path"].startswith("/gw/") or die(f"bad config {cfg!r}")
ok("the config decrypts")

hdr = {"User-Agent": conf["ua"], "X-Auth": conf["auth"]}
# The gateway shows nothing to anyone without all of it.
for bad in ({"User-Agent": conf["ua"]}, {"X-Auth": conf["auth"]}, {"User-Agent": "curl", "X-Auth": conf["auth"]}):
    http("GET", conf["path"] + "/hello", headers=bad)[0] == 404 or die(f"gateway answered {bad}")
http("GET", "/gw/000000000000/hello", headers=hdr)[0] == 404 or die("gateway answered a wrong token")
# Out of order: stage two before the hello.
body = urllib.parse.urlencode({"pw": conf["pw"]}).encode()
http("POST", conf["path"] + "/s2", data=body, headers=hdr)[0] == 409 or die("stage two answered before the hello")
http("GET", conf["path"] + "/hello", headers=hdr)[0] == 200 or die("hello refused")
st, _ = http("POST", conf["path"] + "/s2", data=urllib.parse.urlencode({"pw": "wrong"}).encode(), headers=hdr)
st == 403 or die(f"a wrong password was not refused ({st})")
st, out = http("POST", conf["path"] + "/s2", data=body, headers=hdr)
st == 200 or die(f"stage two refused ({st}) {out}")
flag = re.search(r'"next_key": ?"([^"]+)"', out).group(1)
if args.flag and flag != args.flag:
    die(f"wrong flag {flag!r}")
ok("wrong credentials, wrong order and wrong password are refused; the right ones yield the next key")
print(flag)
