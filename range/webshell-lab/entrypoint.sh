#!/bin/sh
# Stage what the attacker "collected" (the flag is in it), start the web
# server on loopback, and publish it on the unix socket the spawner mounts.
set -eu
umask 077

mkdir -p /tmp/.stage
cat > /tmp/.stage/archive-0412.txt <<STAGED
# staged for upload - do not delete
wp_admin   admin       Ql7!pawpaw-garden
wp_editor  m.okafor    seedswap2026
db_dump    wp_users    3 rows
exfil_key  ${NECROPSY_FLAG:-unset}
STAGED

php -S 127.0.0.1:8080 -t /srv/site >/dev/null 2>&1 &

exec socat UNIX-LISTEN:/run/sock/app.sock,fork,mode=666,unlink-early \
     TCP:127.0.0.1:8080
