The task was to download **http://files.winsvc-telemetry.test/p/kb5034441.exe** and run it.

### The shape of a beacon

- **Check-in.** `POST /gate.php` to `update.winsvc-telemetry.test`, every 60 seconds, with the same small form body: `id=WS-0193&st=idle&n=3`. A machine ID, a status, a counter.
- **Idle reply.** `OK`. The implant sleeps and asks again.
- **Tasking.** On the fifth check-in the reply is JSON: `{"cmd": "download", "url": "...", "run": true}`. That request also says `st=busy`.
- The user agent, `MSIE 6.0; Windows NT 5.1`, is from 2001. Real browsers on that machine send a current one. A hard-coded old user agent is a common implant tell.

`frame.time_delta_displayed` with the POST filter applied shows the interval: 60 seconds, with no jitter. Real browsing does not repeat on a timer.

### What a defender takes from it

Regular intervals, the same path, a tiny body and a tiny reply: that pattern finds the beacon even when the host name looks harmless. The tasking reply gives the next stage's URL to block and to fetch for analysis.
