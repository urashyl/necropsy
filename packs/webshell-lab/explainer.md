The shell is a few lines of PHP. It checks one request header, and if the value matches it runs whatever is in one form field and returns the output. Everything else gets a plain 404, which is why the access log shows a `404` for the first `GET` and `200`s for the `POST`s: whoever scans the site without the header sees nothing.

The obfuscation is the common cheap kind. The header name, its value and the field name are all base64 strings assembled at run time, so a grep for `X-Auth` or `cmd` finds nothing. Decoding them is one line each.

The log tells the rest of the story. The `python-requests` user agent is a script, not a browser. The response sizes (38 bytes, then 1.2 KB, 412 bytes, 9 KB) are a command being run, then directory listings, then a file being read. A file named like an image sitting in `uploads/`, with a dot in front of it, is a classic place for one of these: the folder is writable by the web server, and most people never look inside it.

The stash was in `/tmp/.stage`, on purpose. Attackers stage what they collect in places that are writable and rarely inspected before they copy it out, and `ls` without `-a` skips anything that starts with a dot.

Using a webshell like this against a copy is how responders learn what an attacker could see: what the web user can read, which credentials were within reach, and what to rotate.
