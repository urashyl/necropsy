The answer is `C:\WINDOWS\SYSTEM\WINFAT32.EXE`.

The registry still has every entry the worm made, because the bulletin only said to delete files. Three of them now point at nothing. `MSKernel32`, `Win32DLL` and `WIN-BUGSFIX` all come back `File not found`, so they fail quietly at every boot without running anything.

The rest are what a Windows 98 PC normally has (`ScanRegistry`, `TaskMonitor`, `SystemTray`, `LoadPowerProfile`, `LoadQM`, `SchedulingAgent`), plus Norton and HP's printer monitor. Their files are all there, with dates from when Windows or that software was installed.

All except `WinFAT32`, whose value is `WINFAT32.EXE`. There's no folder in it, so Windows searches the usual places, and the file is in `C:\WINDOWS\SYSTEM`, dated 4 May 2000 at 11:56. That's `WIN-BUGSFIX.exe` after it ran. The worm had set Internet Explorer's start page to the stealer's download, so the next time anyone opened IE that morning, it was saved to `C:\`. When it ran, it looked for a `WinFAT32` value under `Run`, found none, copied itself into the System folder under that name and added the value. Deleting the copy in `C:\`, as the bulletin said to, made no difference.

It's a name meant to be skipped. FAT32 was the file system most Windows 98 machines used, and a `WINFAT32.EXE` in `SYSTEM` looks like it came with Windows. Comparing every startup entry against what should be there is slow, but it catches this. Going down a list of known bad names doesn't.

The small email after each boot is the stealer sending the passwords Windows had cached to an address built into it, which is why changing them didn't help. To actually clean the PC: delete the file and the four worm entries in the registry, reboot, and only then change the passwords.

The stealer's install steps and its `WinFAT32` check are from [F-Secure's description of VBS/LoveLetter](https://www.f-secure.com/v-descs/love). The download is in `regruns()` in [ILOVEYOU: the source](/p/iloveyou-dropped). The PC, the ticket and the listings are made up.
