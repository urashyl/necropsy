# A live command-and-control server and the implant that talks to it. Real code, no
# simulation: the sample is packed Python with a per-player key, and the server speaks
# an encrypted, authenticated protocol (nonce, stream cipher, MAC) that a player has to
# reimplement from the sample before it will say anything. Python's standard library
# only. It runs as an unprivileged user with a read-only root; the spawner gives it no
# network and one writable tmpfs at /tmp.
#
# Contract with the spawner (docs/range.md): listen on /run/sock/app.sock (a unix
# socket, bind-mounted from the host) and read the instance's flag, once, from
# /run/sock/flag.
FROM python:3.12-alpine

RUN apk add --no-cache socat

COPY server.py implant.tmpl /srv/
COPY entrypoint.sh /entrypoint.sh
RUN chmod 0755 /entrypoint.sh
USER 10001:10001
ENTRYPOINT ["/entrypoint.sh"]
