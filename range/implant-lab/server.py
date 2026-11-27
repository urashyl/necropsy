#!/usr/bin/env python3
"""The C2 for implant-lab, and the analyst's workbench around it.

A real server, not a script of canned replies. It speaks the protocol the implant
speaks: every message is `base64url(nonce || ciphertext || mac)`, the ciphertext is
JSON XORed with a SHA-256 keystream, and the MAC is an HMAC-SHA256 truncated to
8 bytes over nonce and ciphertext. The key is per player and exists in the sample
only under two layers (a packed file, then a XORed constant).

Nothing here is reachable except through the lab's unix socket.
"""
import base64
import hashlib
import hmac
import json
import os
import secrets
import threading
import time
import zlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

SOCK_DIR = os.environ.get("LAB_SOCK_DIR", "/run/sock")
TMP = os.environ.get("LAB_TMP", "/tmp")
PORT = int(os.environ.get("LAB_PORT", "8080"))
HERE = os.environ.get("LAB_SRC", os.path.dirname(os.path.abspath(__file__)))

SEED = b"mT9x"  # the sample's rolling-XOR seed for its key constant
FRESH = 600  # seconds a hello's timestamp may be off by
JOB_FILE = "vault/credentials.kdbx"


# ----------------------------------------------------------------- the protocol


def keystream(key, nonce, n):
    out, c = b"", 0
    while len(out) < n:
        out += hashlib.sha256(key + nonce + c.to_bytes(4, "big")).digest()
        c += 1
    return out[:n]


def seal(key, obj, nonce=None):
    pt = json.dumps(obj, separators=(",", ":")).encode()
    nonce = nonce or os.urandom(8)
    ct = bytes(a ^ b for a, b in zip(pt, keystream(key, nonce, len(pt))))
    mac = hmac.new(key, nonce + ct, hashlib.sha256).digest()[:8]
    return base64.urlsafe_b64encode(nonce + ct + mac).decode().rstrip("=")


def unseal(key, text):
    """The decoded message and its nonce, or ValueError."""
    try:
        raw = base64.urlsafe_b64decode(text.strip() + "=" * (-len(text.strip()) % 4))
    except Exception as e:
        raise ValueError("encoding") from e
    if len(raw) < 17:
        raise ValueError("short")
    nonce, ct, mac = raw[:8], raw[8:-8], raw[-8:]
    good = hmac.new(key, nonce + ct, hashlib.sha256).digest()[:8]
    if not hmac.compare_digest(good, mac):
        raise ValueError("mac")
    pt = bytes(a ^ b for a, b in zip(ct, keystream(key, nonce, len(ct))))
    try:
        return json.loads(pt), nonce
    except Exception as e:
        raise ValueError("json") from e


# ------------------------------------------------------------------ the instance


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
        digest = hashlib.sha256(b"necropsy-implant|" + flag.encode()).digest()
        self.key = digest[:16]
        self.bot = "WS-" + digest[16:19].hex().upper()
        self.sessions = {}
        self.seen = set()  # nonces already accepted: no replays
        self.lock = threading.Lock()
        self.sample = self.build_sample()
        self.capture = self.build_capture()

    # The sample: the real implant source, packed, as it was recovered.
    def build_sample(self):
        enc = bytes(b ^ SEED[i % len(SEED)] for i, b in enumerate(self.key)).hex()
        with open(os.path.join(HERE, "implant.tmpl")) as f:
            src = f.read().replace("@@KEY@@", enc).replace("@@BOT@@", self.bot)
        blob = base64.b64encode(zlib.compress(src.encode(), 9)).decode()
        lines = [blob[i : i + 76] for i in range(0, len(blob), 76)]
        body = "\n".join(f'    "{ln}"' for ln in lines)
        return (
            "# helper-update.py - recovered from WS-0412\n"
            "import base64, zlib\n"
            "exec(zlib.decompress(base64.b64decode(\n" + body + "\n)))\n"
        )

    # One captured session from the victim, made with the same key. A test vector,
    # and nothing that can be replayed: its nonces are already spent.
    def build_capture(self):
        t0 = int(time.time()) - 3 * 86400
        msgs = []
        for d, obj in [
            (">", {"bot": self.bot, "host": "WS-0412", "t": t0}),
            ("<", {"sid": "3b0e41d97a52c611", "iv": 600}),
            (">", {"sid": "3b0e41d97a52c611"}),
            ("<", {"jobs": [{"id": 2, "op": "collect", "path": "vault/notes.txt"}]}),
            (">", {"sid": "3b0e41d97a52c611", "job": 2, "data": base64.b64encode(b"standup notes\n").decode()}),
            ("<", {"ack": 2, "next_key": "none"}),
        ]:
            text = seal(self.key, obj)
            self.seen.add(unseal(self.key, text)[1])
            msgs.append((d, text))
        out = ["# ws0412.cap - one beacon cycle, as captured on the wire (3 days ago)",
               "# > implant to server    < server to implant", ""]
        endpoints = ["POST /v3/hb", "reply", "POST /v3/jobs", "reply", "POST /v3/out", "reply"]
        for (d, text), ep in zip(msgs, endpoints):
            out.append(f"{d} {ep}")
            out.append(f"  {text}")
        return "\n".join(out) + "\n"

    # ---- handlers; each returns (status, content_type, body)
    def reply(self, obj):
        return 200, "text/plain", seal(self.key, obj)

    @staticmethod
    def bad():
        return 400, "application/json", '{"e":"bad request"}'

    def post(self, path, text):
        try:
            obj, nonce = unseal(self.key, text)
        except ValueError:
            return self.bad()
        with self.lock:
            if nonce in self.seen:
                return self.bad()
            self.seen.add(nonce)
            if path == "/v3/hb":
                if obj.get("bot") != self.bot or abs(int(obj.get("t", 0)) - time.time()) > FRESH:
                    return self.bad()
                sid = secrets.token_hex(8)
                self.sessions[sid] = {"jobs": False, "done": False}
                return self.reply({"sid": sid, "iv": 600})
            s = self.sessions.get(obj.get("sid"))
            if s is None:
                return 403, "application/json", '{"e":"no session"}'
            if path == "/v3/jobs":
                s["jobs"] = True
                return self.reply({"jobs": [{"id": 3, "op": "collect", "path": JOB_FILE}]})
            if path == "/v3/out":
                if not s["jobs"] or obj.get("job") != 3:
                    return self.bad()
                try:
                    data = base64.b64decode(obj.get("data", ""), validate=True)
                except Exception:
                    return self.bad()
                if not data:
                    return self.bad()
                s["done"] = True
                return self.reply({"ack": 3, "next_key": self.flag})
        return 404, "text/plain", "Not Found"


PAGE = """<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>Sample triage: WS-0412</title>
<style>
body{font-family:system-ui,sans-serif;max-width:44rem;margin:2.5rem auto;padding:0 1rem;color:#1d2330;line-height:1.5}
code,pre{font-family:ui-monospace,monospace;background:#f1f3f7;padding:.1rem .3rem;border-radius:3px}
pre{padding:.7rem;overflow:auto}table{border-collapse:collapse}td,th{border:1px solid #ccd;padding:.3rem .6rem;text-align:left}
</style></head><body>
<h1>Sample triage: WS-0412</h1>
<p>Incident 4417. A finance workstation made an HTTPS request every ten minutes to an
unfamiliar font-hosting domain. EDR kept the file it ran from, and the network team kept
one cycle of the traffic. This page is the case file, and <strong>this server is the
operator's server</strong> as it was when we took it down: it answers whoever speaks its
protocol.</p>
<h2>Evidence</h2>
<table>
<tr><th>Item</th><th></th></tr>
<tr><td><a href="/samples/helper-update.py.txt">helper-update.py.txt</a></td><td>The file EDR kept. sha256 <code>@@SHA@@</code></td></tr>
<tr><td><a href="/captures/ws0412.cap">ws0412.cap</a></td><td>One captured beacon cycle, three days old</td></tr>
</table>
<h2>Analyst notes</h2>
<ul>
<li>The traffic is base64url, never plain JSON, and every POST body is different even for the same request.</li>
<li>The operator rotates the key it shares with the implant after each collection. We believe they are careless about what they put in that message.</li>
</ul>
</body></html>
"""


def make_handler(lab):
    sha = hashlib.sha256(lab.sample.encode()).hexdigest()

    class H(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.0"
        server_version = "triage"

        def log_message(self, *a):
            pass

        def send(self, status, ctype, body, extra=None):
            data = body.encode()
            self.send_response(status)
            self.send_header("Content-Type", ctype + "; charset=utf-8")
            self.send_header("Content-Length", str(len(data)))
            for k, v in (extra or {}).items():
                self.send_header(k, v)
            self.end_headers()
            self.wfile.write(data)

        def do_GET(self):
            p = self.path.split("?")[0]
            if p == "/":
                self.send(200, "text/html", PAGE.replace("@@SHA@@", sha))
            elif p == "/samples/helper-update.py.txt":
                self.send(200, "text/plain", lab.sample,
                          {"Content-Disposition": 'attachment; filename="helper-update.py.txt"'})
            elif p == "/captures/ws0412.cap":
                self.send(200, "text/plain", lab.capture)
            else:
                self.send(404, "text/plain", "Not Found")

        def do_POST(self):
            n = int(self.headers.get("Content-Length") or 0)
            if n > 8192:
                return self.send(413, "text/plain", "too large")
            body = self.rfile.read(n).decode("utf-8", "replace")
            self.send(*lab.post(self.path.split("?")[0], body))

    return H


def main():
    lab = Lab(read_flag())
    srv = ThreadingHTTPServer(("127.0.0.1", PORT), make_handler(lab))
    srv.daemon_threads = True
    srv.serve_forever()


if __name__ == "__main__":
    main()
