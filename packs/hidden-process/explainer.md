The answer is `/usr/lib/x86_64-linux-gnu/libc.so.7`.

Compare the two outputs. Normal `ps` has no pid 966, but busybox's does: a `www-data` process calling itself `kworker/1:2`. Real kworkers are kernel threads, owned by root and shown in brackets, so this one picked the name to blend in. `ss` shows the connection to 203.0.113.200 with no process next to it, and busybox `netstat` shows the same socket belonging to 966. Its executable is a deleted file in PHP's session folder, which points to how it got there.

So the normal tools are being lied to, and busybox isn't. The difference is that this busybox is statically linked. `ps`, `ss`, `cat` and `ls` all load shared libraries when they start, and on Linux, anything listed in `/etc/ld.so.preload` is loaded into every dynamically linked program first. A library there can wrap `readdir()` so that `/proc/966` never shows up in a directory listing, and wrap `open()` so that the file listing it can't be read. That's why plain `cat /etc/ld.so.preload` says the file doesn't exist and busybox `cat` prints it.

What it lists is `/usr/lib/x86_64-linux-gnu/libc.so.7`. The name is meant to look like part of glibc, which really is `libc.so.6`. It's 17 KB next to a 1.9 MB real libc, no package owns it, it was written on 19 November, and it's mapped into process 966 too.

This is why deleting the cron job didn't help. The hider isn't the thing making the connection, but it keeps the thing making the connection out of sight. Real Linux malware has done this for years, Symbiote in 2022 being a well-known example.

To clean it, work from a system you trust: boot from rescue media or take the disk to another machine, rather than trusting tools on this one. Remove `/etc/ld.so.preload` and the library, find how process 966 is restarted, and look at the PHP app that let `www-data` drop it.
