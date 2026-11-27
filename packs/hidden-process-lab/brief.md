A gym's web server was flagged for odd outbound traffic. The admin checked with `ps` and `top` and saw nothing, so we reproduced the host and kept its remote support console.

The console runs any command you send it, but only with the support header (see `notes.txt`). Work out why the usual tools show nothing, and find the process that is still running. It holds a key in its environment. Submit that `EXFIL_KEY` as the flag.
