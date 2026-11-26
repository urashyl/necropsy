#!/bin/sh
# Stage what the attacker "collected" (the flag is in it), start the web
# server on loopback, and publish it on the unix socket the spawner mounts.
set -eu
umask 077

# The flag arrives as a file in the socket directory, read once and deleted. It is
# never in the environment, where anyone with a shell could read it from /proc/1.
FLAG="$(cat /run/sock/flag 2>/dev/null || echo unset)"
rm -f /run/sock/flag

mkdir -p /tmp/.stage
cat > /tmp/.stage/archive-0412.txt <<STAGED
# staged for upload - do not delete
wp_admin   admin       Ql7!pawpaw-garden
wp_editor  m.okafor    seedswap2026
db_dump    wp_users    3 rows
exfil_key  ${FLAG}
STAGED

php -S 127.0.0.1:8080 -t /srv/site >/dev/null 2>&1 &

exec socat UNIX-LISTEN:/run/sock/app.sock,fork,mode=666,unlink-early \
     TCP:127.0.0.1:8080
