The download is `GET /s/8fk2Lq/invoice_0921.zip` to **dl.quickdocs-share.test**, a few seconds after the user opened their webmail.

### How to find it

- `http.request` shows only the requests: five of them, so the capture is small enough to read row by row.
- The request line (`GET /s/8fk2Lq/invoice_0921.zip HTTP/1.1`) has the path. The `Host:` header has the server name, which is what a browser asked DNS for just before. The DNS query in the frames above it says the same name resolved to 192.0.2.77.
- Follow HTTP stream on that request shows both sides: the request headers, and the response starting with `PK\x03\x04`, the first bytes of every zip file.
- The `Referer` header of the download is the webmail inbox. That is the chain: email, link, file.

### What a defender takes from it

The host, the path and the IP are indicators for the proxy and the firewall. The Referer ties the download to an email, so the next step is to find that message and everyone else who received it.
