The answer is `WINFAT32.EXE`.

The stealer arrived as `WIN-BUGSFIX.EXE`, a name chosen to sound like a Windows fix. Before doing anything else it looked for a `WinFAT32` entry under the `Run` registry key. If there was none, it copied itself into the System directory as **`WINFAT32.EXE`** and added the `Run` entry for that copy, so it started on every boot. Again the name does the hiding: FAT32 is the file system many Windows 98 machines used, so `WINFAT32` looks like part of the system.

Two details are worth noticing, because they are how malware tells itself *I have already run here*:

- **A marker key.** The check for `WinFAT32` under `Run` is a guard: if it exists, the program is already installed and skips the setup. Analysts use the same marker as an indicator.
- **Two names for one program.** `WIN-BUGSFIX.EXE` is how it arrives and `WINFAT32.EXE` is how it lives. Cleaning only the first leaves the one that runs.

When you investigate something like this, the checklist is the same:

| Question | Where it showed up here |
| --- | --- |
| How did it get here? | the email attachment, then a download |
| How does it survive a reboot? | a `Run` registry entry for `WINFAT32.EXE` |
| What does it take? | saved passwords, sent out by email |
| What marks an infected machine? | `WINFAT32.EXE` in the System directory, a `WinFAT32` subkey under `Run` |

The marker key and the file name are exactly the **indicators of compromise** that were shared in those first hours, and you can search for them across a whole network.

Sources: [F-Secure description of Email-Worm:VBS/LoveLetter](https://www.f-secure.com/v-descs/love) and [Wikipedia: ILOVEYOU](https://en.wikipedia.org/wiki/ILOVEYOU). The artifacts above are reconstructed from those descriptions, not captured from a live run. The worm's own source is the file in [ILOVEYOU: the source](/p/iloveyou-dropped), from the formatted, commented copy archived at [github.com/ferristotle/ILOVEYOU](https://github.com/ferristotle/ILOVEYOU).
