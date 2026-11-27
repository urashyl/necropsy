A loader infecting a few hundred machines derives a new command-server domain every day, and its operator registers only that day's name. Anyone who works out the algorithm can predict the domains. Our sinkhole does exactly that and answers for them.

The generator is `netcheck.js.txt`. The incident notes are `victim.txt`. Find the domain it would use on that workstation on **24 December 2026** (UTC) and ask the sinkhole about it (`/lookup?domain=<name>`). Submit the flag it returns.
