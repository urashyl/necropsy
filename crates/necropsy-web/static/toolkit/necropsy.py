"""Helpers for the workspace notebook.

    from necropsy import request, file

    data = await file("capture.pcap")          # a puzzle file, as bytes
    r = await request("POST", "/api/v2/sync",   # to your own instance
                      headers={"User-Agent": "Pebble/2.4"},
                      data={"d": "..."})
    print(r.status, r.headers, r.text)

request() sends through the same connection as the request console, so any
header can be set, User-Agent and Cookie included. On your own computer the
same code is requests.request(method, url, headers=..., data=...) and
open(name, "rb").read().
"""

import json as _json
from urllib.parse import urlencode, urlsplit

import necropsy_bridge as _bridge
from pyodide.ffi import to_js

USER_AGENT = "python-requests/2.32.3"


class Response:
    """An HTTP response: status, reason, headers (a dict, names in lower case),
    content (bytes) and text."""

    def __init__(self, raw):
        head, _, body = raw.partition(b"\r\n\r\n")
        lines = head.decode("latin-1").split("\r\n")
        parts = lines[0].split(" ", 2)
        self.status = int(parts[1]) if len(parts) > 1 and parts[1].isdigit() else 0
        self.reason = parts[2] if len(parts) > 2 else ""
        self.headers = {}
        for line in lines[1:]:
            name, _, value = line.partition(":")
            self.headers[name.strip().lower()] = value.strip()
        self.content = body
        self.raw = raw

    @property
    def text(self):
        return self.content.decode("utf-8", "replace")

    def json(self):
        return _json.loads(self.content)

    def __repr__(self):
        return f"<Response {self.status} {self.reason}, {len(self.content)} bytes>"


def _target(url):
    host = _bridge.instance_host()
    if not host:
        raise RuntimeError("Start the instance first: request() only reaches your own instance.")
    if url.startswith("/"):
        return host, url
    parts = urlsplit(url)
    if parts.scheme not in ("http", "https") or not parts.netloc:
        raise ValueError(f"{url!r} is neither a path like /api nor an http(s) URL.")
    if parts.netloc.lower() != host.lower():
        raise ValueError(f"request() only reaches your instance, {host}.")
    path = parts.path or "/"
    return host, path + (f"?{parts.query}" if parts.query else "")


def build(method, url, headers=None, data=None, json=None, params=None):
    """The raw request request() would send, as bytes."""
    host, target = _target(url)
    if params:
        target += ("&" if "?" in target else "?") + urlencode(params)
    body = b""
    extra = {}
    if json is not None:
        body = _json.dumps(json).encode()
        extra["Content-Type"] = "application/json"
    elif isinstance(data, dict):
        body = urlencode(data).encode()
        extra["Content-Type"] = "application/x-www-form-urlencoded"
    elif isinstance(data, str):
        body = data.encode()
    elif data is not None:
        body = bytes(data)
    given = {k.lower() for k in (headers or {})}
    lines = [f"{method.upper()} {target} HTTP/1.1"]
    for name, value in [("Host", host), ("User-Agent", USER_AGENT), ("Accept", "*/*")]:
        if name.lower() not in given:
            lines.append(f"{name}: {value}")
    for name, value in (headers or {}).items():
        lines.append(f"{name}: {value}")
    if body or method.upper() in ("POST", "PUT", "PATCH"):
        if "content-length" not in given:
            lines.append(f"Content-Length: {len(body)}")
    for name, value in extra.items():
        if name.lower() not in given:
            lines.append(f"{name}: {value}")
    return ("\r\n".join(lines) + "\r\n\r\n").encode("latin-1") + body


async def request(method, url, headers=None, data=None, json=None, params=None):
    """Sends a request to the instance and returns a Response."""
    raw = build(method, url, headers=headers, data=data, json=json, params=params)
    reply = await _bridge.request(to_js(raw))
    return Response(bytes(reply.to_py()))


async def file(name):
    """A file of this puzzle, as bytes."""
    data = await _bridge.file(name)
    return bytes(data.to_py())
