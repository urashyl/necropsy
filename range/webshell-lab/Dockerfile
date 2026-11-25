# A small WordPress-looking site that has been compromised: a working webshell
# sits in the uploads folder. Runs as an unprivileged user with a read-only
# root; the spawner gives it no network and one writable tmpfs at /tmp.
#
# Contract with the spawner (docs/range.md): listen on /run/sock/app.sock
# (a unix socket, bind-mounted from the host) and read the instance's flag
# from $NECROPSY_FLAG.
FROM php:8.3-cli-alpine
RUN apk add --no-cache socat
COPY site /srv/site
COPY entrypoint.sh /entrypoint.sh
RUN chmod 0755 /entrypoint.sh
USER 10001:10001
ENTRYPOINT ["/entrypoint.sh"]
