The answer is `Win32DLL.vbs`.

`regruns()` writes two values:

```vbscript
regcreate "...\CurrentVersion\Run\MSKernel32", dirsystem & "\MSKernel32.vbs"
regcreate "...\CurrentVersion\RunServices\Win32DLL", dirwin & "\Win32DLL.vbs"
```

`dirwin` is `GetSpecialFolder(0)`, the Windows folder, and a few lines into `main()` the script copied itself there as `Win32DLL.vbs`. The other two copies went to the System folder.

On Windows 95 and 98, `RunServices` starts things before anyone logs in, and `Run` after. Using both means the worm is running again even if nobody gets past the login prompt. The names do the rest. `MSKernel32` and `Win32DLL` sound like parts of Windows, and someone scrolling through `C:\WINDOWS` in 2000 would have gone straight past them.

Now that you have it open, the rest is worth reading. `spreadtoemail()` goes through every Outlook address list and sends one message per contact, which is why it arrived from people you knew. The next puzzle is about that. `infectfiles()` overwrites scripts and pictures on every drive with copies of itself and hides MP3s, and that was the real damage. And `regruns()` sets Internet Explorer's start page to one of four URLs for a password stealer, `WIN-BUGSFIX.exe`, which the puzzle after that follows.

This formatted, commented copy is from [github.com/ferristotle/ILOVEYOU](https://github.com/ferristotle/ILOVEYOU), which credits [cexx.org](http://www.cexx.org/loveletter.htm). There's background on [Wikipedia](https://en.wikipedia.org/wiki/ILOVEYOU). It needs Windows Script Host and Outlook from that era, and does nothing on a current system.
