# A web server with something running on it that the normal tools cannot see: a
# process hidden by a library in /etc/ld.so.preload, as real Linux malware does.
# Real thing, no simulation: ps, top and ls really do lie, and only reading /proc
# directly shows the truth. It runs as an unprivileged user with a read-only root;
# the spawner gives it no network and one writable tmpfs at /tmp.
#
# Contract with the spawner (docs/range.md): listen on /run/sock/app.sock (a unix
# socket, bind-mounted from the host) and read the instance's flag, once, from
# /run/sock/flag.
FROM debian:bookworm-slim

RUN apt-get update \
 && apt-get install -y --no-install-recommends php-cli procps socat gcc libc6-dev \
 && rm -rf /var/lib/apt/lists/*

# The hider, built, then the compiler removed. The preload file is written last, so
# nothing above runs with it in place.
COPY libc7.c /tmp/libc7.c
RUN gcc -shared -fPIC -O2 -o /usr/lib/libc.so.7 /tmp/libc7.c -ldl \
 && rm /tmp/libc7.c \
 && apt-get purge -y gcc libc6-dev \
 && apt-get autoremove -y \
 && mkdir -p /usr/lib/.sysmon \
 && cp /bin/sleep /usr/lib/.sysmon/sysupd \
 && echo /usr/lib/libc.so.7 > /etc/ld.so.preload

COPY site /srv/site
COPY entrypoint.sh /entrypoint.sh
RUN chmod 0755 /entrypoint.sh
USER 10001:10001
ENTRYPOINT ["/entrypoint.sh"]
