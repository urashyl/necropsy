A loader that has infected a few hundred machines never uses the same command server twice: each day it derives a new domain from the date, and its operator registers only that day's name. That makes blocking by domain useless, but it also means that **whoever works out the algorithm can predict the domains**, and a researcher can register them first.

That is what our sinkhole does. It answers for domains a sample would use. We recovered the generator as `netcheck.py.txt` and put the sinkhole's console on your instance. Start it below.

Find out which domain the sample would use on **24 December 2026** (UTC), and ask the sinkhole about it (`/lookup?domain=<name>`). Submit the flag it returns.
