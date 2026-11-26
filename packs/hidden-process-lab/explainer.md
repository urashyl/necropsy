The key is in the environment of a process called `sysupd` that `ps` never shows, and the way to it is `/proc`.

Dynamically linked programs such as `ps`, `top` and `ls` load everything named in `/etc/ld.so.preload` before they start. Here it lists `/usr/lib/libc.so.7`, whose name is chosen to look like part of glibc (the real one is `libc.so.6`). That library wraps `readdir()` so the process's directory never appears in a listing of `/proc`. The tools aren't lying about the process list, they are being made to see a different one.

The wrapper only hides names from listings. The process still exists, and `/proc/<pid>/cmdline` and `/proc/<pid>/environ` still open if you ask for them by number, so counting through pids in a shell loop finds the one whose command line doesn't belong. Its environment holds `EXFIL_KEY`, which is the flag.

Things that point at the hider without any of that: `cat /etc/ld.so.preload` (one line), the library's size next to the real libc, and `grep sysupd` finding nothing in `ps` while the pid is still answering. Real Linux malware has done this for years; Symbiote in 2022 is a well-known example. To clean it, boot from trusted media, remove the preload file and the library, and look for what restarts the process.
