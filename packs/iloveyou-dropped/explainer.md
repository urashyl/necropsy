The answer is `Win32DLL.vbs`.

In `main()` the worm resolves three folders:

```vbscript
Set dirwin    = fso.GetSpecialFolder(0)   ' the Windows folder, e.g. C:\WINDOWS
Set dirsystem = fso.GetSpecialFolder(1)   ' the System folder,  e.g. C:\WINDOWS\SYSTEM
Set dirtemp   = fso.GetSpecialFolder(2)   ' the Temp folder
```

then copies itself three times:

```vbscript
c.Copy(dirsystem & "\MSKernel32.vbs")             ' -> System
c.Copy(dirwin    & "\Win32DLL.vbs")               ' -> Windows   <- the one asked for
c.Copy(dirsystem & "\LOVE-LETTER-FOR-YOU.TXT.vbs")' -> System
```

The whole puzzle is knowing that `GetSpecialFolder(0)` is the Windows directory and `(1)` is the System directory inside it. The copy built from `dirwin` is `Win32DLL.vbs`.

The names are chosen to blend in: `MSKernel32` and `Win32DLL` read like parts of Windows (`KERNEL32.DLL`, system DLLs), so a glance at the folder would not flag them. `regruns()` then writes `Run` and `RunServices` registry keys pointing at those two copies, so they execute on every boot — which is why deleting the emailed attachment never cleaned an infected PC.

Worth tracing in the rest of the file, now that you have it:

- **`spreadtoemail()`** opens `Outlook.Application`, walks every entry in every MAPI address list, and mails each one a copy with the subject `ILOVEYOU`. A `WAB` registry marker per address stops it mailing the same person twice. This is why each copy arrived from someone you knew.
- **`infectfiles()`** overwrites `.vbs`/`.vbe` files with itself, replaces `.js`/`.css`/`.hta` and image/audio files with `.vbs` copies (destroying the originals), and drops a malicious mIRC `script.ini`. This is the part that did real damage.
- **`regruns()`** also points Internet Explorer's start page at a `skyinet.net` URL that downloaded a separate password-stealing trojan (`WIN-BUGSFIX.exe`).
- **`html()`** writes an HTML copy carrying the author's own credit string.

How to read a sample like this: start at the entry point (`main()` here), list what it calls, and for each routine ask the four questions — how it installs, how it persists, how it spreads, and what it damages. The comments in this copy were added for teaching; the real thing was one terse block headed `On Error Resume Next` so that every failure was swallowed silently.

Source: the worm's source is widely archived; this formatted, commented copy is from [github.com/ferristotle/ILOVEYOU](https://github.com/ferristotle/ILOVEYOU) (which credits [cexx.org](http://www.cexx.org/loveletter.htm)). Background: [Wikipedia: ILOVEYOU](https://en.wikipedia.org/wiki/ILOVEYOU). VBScript worms that drive Outlook's address book on Windows 98/2000 have no attack surface on any current system, which is why copies like this are kept as plain text for study.
