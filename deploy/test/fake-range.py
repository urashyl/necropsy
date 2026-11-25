#!/usr/bin/env python3
"""A stand-in for the range VM's spawner, for the installer tests: /healthz answers,
and /v1/instances answers 200 only for the right token (else 401).
Usage: fake-range.py <token>; prints the port it listens on, then serves."""
import http.server
import sys

TOKEN = sys.argv[1]


class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/healthz":
            code, body = 200, b"ok"
        elif self.path == "/v1/instances" and self.headers.get("X-Necropsy-Token") == TOKEN:
            code, body = 200, b"[]"
        else:
            code, body = 401, b"a valid X-Necropsy-Token is required"
        self.send_response(code)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


srv = http.server.HTTPServer(("127.0.0.1", 0), H)
print(srv.server_address[1], flush=True)
srv.serve_forever()
