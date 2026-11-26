# A real compiled ELF beacon and the gateway it talks to. The binary is built here from
# implant.c (then the compiler is removed); the server patches each player's own
# configuration into a copy in memory, encrypted with RC4 under a key the program
# builds at run time. Real binary analysis, static or dynamic, with nothing simulated.
# It runs as an unprivileged user with a read-only root; the spawner gives it no
# network and one writable tmpfs at /tmp.
#
# Contract with the spawner (docs/range.md): listen on /run/sock/app.sock (a unix
# socket, bind-mounted from the host) and read the instance's flag, once, from
# /run/sock/flag.
FROM debian:bookworm-slim

RUN apt-get update \
 && apt-get install -y --no-install-recommends python3 socat gcc libc6-dev \
 && rm -rf /var/lib/apt/lists/*

COPY implant.c /tmp/implant.c
RUN gcc -O0 -s -no-pie -fno-stack-protector -o /srv-implant.bin /tmp/implant.c \
 && rm /tmp/implant.c \
 && apt-get purge -y gcc libc6-dev \
 && apt-get autoremove -y \
 && mkdir /srv && mv /srv-implant.bin /srv/implant.bin

COPY server.py /srv/server.py
COPY entrypoint.sh /entrypoint.sh
RUN chmod 0755 /entrypoint.sh
USER 10001:10001
ENTRYPOINT ["/entrypoint.sh"]
