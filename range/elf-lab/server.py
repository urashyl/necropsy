#!/usr/bin/env python3
"""The gateway for elf-lab, and the analyst's workbench around it.

The player's configuration (gateway path, auth value, stage-two password) is
derived from the instance's flag, RC4-encrypted under the key the binary assembles
at run time, and patched into a copy of the ELF in memory. The gateway answers only
a client that has it.
"""
import hashlib
import json
import os
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs

SOCK_DIR = os.environ.get("LAB_SOCK_DIR", "/run/sock")
PORT = int(os.environ.get("LAB_PORT", "8080"))
BINARY = os.environ.get("LAB_BINARY", "/srv/implant.bin")

MARKER = bytes([0x4E, 0x43, 0x46, 0x47, 0xA5, 0x5A, 0xC3, 0x3C])
RC4_KEY = b"vendorsync27"
UA = "Mozilla/5.0 (X11; Linux x86_64) svcupd/2.1"


def rc4(key, data):
    s = list(range(256))
    j = 0
    for i in range(256):
        j = (j + s[i] + key[i % len(key)]) & 255
        s[i], s[j] = s[j], s[i]
    i = j = 0
    out = bytearray()
    for b in data:
        i = (i + 1) & 255
        j = (j + s[i]) & 255
        s[i], s[j] = s[j], s[i]
        out.append(b ^ s[(s[i] + s[j]) & 255])
    return bytes(out)


def read_flag():
    path = os.path.join(SOCK_DIR, "flag")
    try:
        with open(path) as f:
            flag = f.read().strip()
    except OSError:
        flag = "unset"
    try:
        os.remove(path)
    except OSError:
        pass
    return flag


class Lab:
    def __init__(self, flag):
        self.flag = flag
        d = hashlib.sha256(b"necropsy-elf|" + flag.encode()).hexdigest()
        self.token, self.auth, self.pw = d[:12], d[12:28], d[28:40]
        self.hello = False
        self.lock = threading.Lock()
        self.binary = self.patch()

    def config(self):
        return (f"path=/gw/{self.token}\nauth={self.auth}\nua={UA}\npw={self.pw}\n").encode()

    def patch(self):
        with open(BINARY, "rb") as f:
            blob = bytearray(f.read())
        at = blob.index(MARKER)
        cfg = rc4(RC4_KEY, self.config())
        assert 0 < len(cfg) <= 240
        blob[at + 8 : at + 10] = len(cfg).to_bytes(2, "little")
        blob[at + 10 : at + 10 + len(cfg)] = cfg
        return bytes(blob)

    def gateway(self, method, path, headers, form):
        """(status, body) for /gw/<token>/..., a plain 404 for anything else."""
        notfound = (404, "Not Found")
        parts = path.split("/")
        if len(parts) != 4 or parts[1] != "gw" or parts[2] != self.token:
            return notfound
        if headers.get("user-agent") != UA or headers.get("x-auth") != self.auth:
            return notfound
        with self.lock:
            if method == "GET" and parts[3] == "hello":
                self.hello = True
                return 200, json.dumps({"ok": True, "next": f"/gw/{self.token}/s2", "interval": 900})
            if method == "POST" and parts[3] == "s2":
                if not self.hello:
                    return 409, json.dumps({"e": "no session"})
                if form.get("pw") != self.pw:
                    return 403, json.dumps({"e": "denied"})
                return 200, json.dumps({"campaign": "svcupd-ru-04", "next_key": self.flag})
        return notfound


PAGE = """<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>Sample triage: svcupd</title>
<style>
body{font-family:system-ui,sans-serif;max-width:44rem;margin:2.5rem auto;padding:0 1rem;color:#1d2330;line-height:1.5}
code,pre{font-family:ui-monospace,monospace;background:#f1f3f7;padding:.1rem .3rem;border-radius:3px}
pre{padding:.7rem;overflow:auto}table{border-collapse:collapse}td,th{border:1px solid #ccd;padding:.3rem .6rem;text-align:left}
</style></head><body>
<h1>Sample triage: svcupd</h1>
<p>Incident 5120. A build server ran a binary called <code>svcupd</code> out of
<code>/var/tmp</code>. It was gone by the next morning; EDR kept a copy. The network team
saw one thing: lookups for a name that never resolved. <strong>This server is the
operator's gateway</strong> as it was when we took it down: it answers a client that
knows its path and credentials, and nobody else.</p>
<h2>Evidence</h2>
<table>
<tr><th>Item</th><th></th></tr>
<tr><td><a href="/samples/svcupd.bin">svcupd.bin</a></td><td>The binary: 64-bit ELF, x86-64, dynamically linked, stripped. sha256 <code>@@SHA@@</code></td></tr>
<tr><td><a href="/notes/network.txt">network.txt</a></td><td>What the network team saw, redacted</td></tr>
</table>
<h2>Analyst notes</h2>
<ul>
<li><code>strings</code> shows a format string for an HTTP request and almost nothing else. No domain, no path, no key.</li>
<li>The binary refuses to run under a debugger, and does nothing visible when run normally.</li>
<li>The gateway name ends in <code>.invalid</code>, so it does not resolve. Use this server instead.</li>
<li>Run it only somewhere you can throw away. You will not need to run it to solve this.</li>
</ul>
</body></html>
"""

NOTES = """network.txt - redacted by the SOC before sharing

2026-11-21 02:14:07  build-04  DNS A? gw.cdn-metrics.invalid           NXDOMAIN
2026-11-21 02:14:08  build-04  DNS A? gw.cdn-metrics.invalid           NXDOMAIN
(repeats every 15 minutes until 2026-11-22 06:02)

Request the operator's own tooling is known to make against this gateway
(from a previous incident, values removed):

  GET /gw/[REDACTED]/hello HTTP/1.1
  Host: gw.cdn-metrics.invalid
  User-Agent: [REDACTED]
  X-Auth: [REDACTED]

  -> 200 {"ok":true,"next":"/gw/[REDACTED]/s2","interval":900}

  POST /gw/[REDACTED]/s2 HTTP/1.1
  (same headers)
  Content-Type: application/x-www-form-urlencoded

  pw=[REDACTED]

  -> 200 {"campaign":"...","next_key":"[REDACTED]"}

Note: the second request is refused unless the first has been made.
"""


def make_handler(lab):
    sha = hashlib.sha256(lab.binary).hexdigest()

    class H(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.0"
        server_version = "triage"

        def log_message(self, *a):
            pass

        def send(self, status, ctype, body, extra=None):
            data = body if isinstance(body, bytes) else body.encode()
            self.send_response(status)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(data)))
            for k, v in (extra or {}).items():
                self.send_header(k, v)
            self.end_headers()
            self.wfile.write(data)

        def headers_lower(self):
            return {k.lower(): v for k, v in self.headers.items()}

        def do_GET(self):
            p = self.path.split("?")[0]
            if p == "/":
                self.send(200, "text/html; charset=utf-8", PAGE.replace("@@SHA@@", sha))
            elif p == "/samples/svcupd.bin":
                self.send(200, "application/octet-stream", lab.binary,
                          {"Content-Disposition": 'attachment; filename="svcupd.bin"'})
            elif p == "/notes/network.txt":
                self.send(200, "text/plain; charset=utf-8", NOTES)
            else:
                s, b = lab.gateway("GET", p, self.headers_lower(), {})
                self.send(s, "application/json" if s != 404 else "text/plain", b)

        def do_POST(self):
            n = int(self.headers.get("Content-Length") or 0)
            if n > 4096:
                return self.send(413, "text/plain", "too large")
            body = self.rfile.read(n).decode("utf-8", "replace")
            form = {k: v[0] for k, v in parse_qs(body).items()}
            s, b = lab.gateway("POST", self.path.split("?")[0], self.headers_lower(), form)
            self.send(s, "application/json" if s != 404 else "text/plain", b)

    return H


def main():
    lab = Lab(read_flag())
    srv = ThreadingHTTPServer(("127.0.0.1", PORT), make_handler(lab))
    srv.daemon_threads = True
    srv.serve_forever()


if __name__ == "__main__":
    main()
