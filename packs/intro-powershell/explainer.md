The answer is `ms-office-addins.cdn-edge.invalid`.

Two of the PowerShell runs have `AgentExecutor.exe` as their parent. That's Intune running its inventory scripts, half an hour apart. Decode them if you want to check: they write OS and disk details to `C:\ProgramData`.

The third one, pid 6488, was started by `cmd.exe`, and that `cmd.exe` was started by `WINWORD.EXE` four minutes after Word opened `Invoice_3382.doc` out of Outlook's attachment cache. Word doesn't open command prompts by itself, so a macro did it. Searching for `parent=WINWORD.EXE image=powershell.exe` misses it, because of the `cmd.exe` in the middle.

`-nop -w 1 -enc` means no profile, hidden window and an encoded command. `-enc` is base64 of UTF-16LE:

```bash
echo '<the -enc string>' | base64 -d | iconv -f UTF-16LE -t UTF-8
```

That gives you a second layer: a base64 blob that gets gunzipped and handed to `IEX`. Do what it does, but print the result instead of running it:

```bash
echo '<the FromBase64String blob>' | base64 -d | gunzip
```

Inside is the downloader. It pretends to be Office in its user agent, uses the system proxy with the user's own credentials so it works on a corporate network, and runs whatever `https://ms-office-addins.cdn-edge.invalid/16.0/telemetry.ps1` sends back without writing it to disk.

The `csc.exe` at 09:22 is the C# compiler, started by the same PowerShell. Windows PowerShell compiles C# on the fly when a script uses `Add-Type`, and a lot of attack tooling does. That's a good sign the download worked and the second script ran.

The hostname is chosen to look like Microsoft in a proxy log. Search the proxy logs for it anyway. Other machines that opened the same invoice will have contacted it too.
