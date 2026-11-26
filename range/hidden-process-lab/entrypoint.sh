#!/bin/sh
# Start the process nobody can see (its environment holds the key the puzzle asks
# for), the web server on loopback, and publish it on the unix socket the spawner
# mounts.
set -eu

# The key arrives as a file in the socket directory, read once and deleted. It is
# never in this shell's environment or its children's, where anyone with a shell
# could read it from /proc: the only process that has it is the hidden one.
KEY="$(cat /run/sock/flag 2>/dev/null || echo unset)"
rm -f /run/sock/flag

# A renamed copy of sleep, standing in for the malware. /usr/lib/libc.so.7, loaded
# into every process by /etc/ld.so.preload, hides it from /proc listings.
EXFIL_KEY="$KEY" /usr/lib/.sysmon/sysupd 604800 >/dev/null 2>&1 &

php -S 127.0.0.1:8080 -t /srv/site >/dev/null 2>&1 &

exec socat UNIX-LISTEN:/run/sock/app.sock,fork,mode=666,unlink-early \
     TCP:127.0.0.1:8080
