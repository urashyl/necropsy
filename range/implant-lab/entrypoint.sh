#!/bin/sh
# The C2 on loopback, published on the unix socket the spawner mounts. The server
# reads the flag file itself, once, and deletes it: it is never in an environment.
set -eu

python3 /srv/server.py &

exec socat UNIX-LISTEN:/run/sock/app.sock,fork,mode=666,unlink-early \
     TCP:127.0.0.1:8080
