The answer is `203.0.113.77`.

Ten logins worked. Seven used a key: `mitch` from 198.51.100.20, and the nightly `backup` job. Two are `ana` with a password from the office address next door, and on the 19th she gets it wrong once and right six seconds later on the same connection. That's a typo, not an attack.

The tenth is `deploy` from 203.0.113.77 at 02:14 on the 20th. In the minute and a half before that, the same address tried `postgres`, `admin`, `root`, `git`, `jenkins` and a handful of others, then `deploy` three times. The fourth password for `deploy` worked.

If you answered 192.0.2.15, you found the loudest address in the file. It failed over a hundred times and never got in. Every server with port 22 open gets traffic like that, so counting failures will always lead you to a scanner. Start from what succeeded and ask whether it should have.

```bash
grep Accepted auth.log
grep 203.0.113.77 auth.log
```

On a real server, you'd end that session, then go through what `deploy` did in its 27 minutes: shell history, its crontab, and `~/.ssh/authorized_keys`, which is where a second way back in usually goes. Then change the password and set `PasswordAuthentication no`.
