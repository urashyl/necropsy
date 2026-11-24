The answer is `203.0.113.77`.

Three addresses appear. `198.51.100.20` logged in once, with a key (`Accepted publickey`) as `mitch`, which is normal. `192.0.2.15` tried hardest, with a dozen failed passwords, but never got in. `203.0.113.77` tried a list of common user names (`postgres`, `admin`, `test`, `git`, `ftp`...) and finally got in as `deploy` with a password (`Accepted password`).

The pattern to look for is **failures, then a success from the same address**. Most failures are background noise from scanners, so counting failures finds the loudest address, not the one that matters. The success after failures is the incident.

With the log in a file, you can find it without reading every line:

```bash
grep Accepted auth.log                       # every successful login
grep 'Failed password' auth.log \
  | grep -o 'from [0-9.]*' | sort | uniq -c | sort -rn   # failures per address
```

Then connect the two lists: the address with an `Accepted password` line that also has failures above it.

What you would do next on a real server: end the `deploy` session, check what it ran (`last`, shell history, new cron jobs and SSH keys), change that password, and turn off password logins so only keys work.
