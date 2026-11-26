A `.bat` that runs `powershell -enc` is a very common first stage. The encoded part is **not** encryption: it is base64 of the script as UTF-16LE text, which is what PowerShell's `-EncodedCommand` expects. Decoding is `base64 -d | iconv -f utf-16le`, and it gives you a small downloader that fetches a second script with `IEX` (Invoke-Expression).

Two behaviours are worth remembering. The downloader sets a fixed User-Agent, and the server returns a plain 404 to anyone else: that is how operators hide the next stage from scanners and from analysts who `curl` the URL without reading the sample. And the stages are chained: stage one builds the path of stage two from a base64 string and sends a marker header, and the server only serves stage two to someone who has already fetched stage one. Skipping straight to the end fails for the same reason.

Stage one also checks `UserDomainName -eq 'WORKGROUP'` and quits, a sandbox check: analysis machines are often not domain-joined.

Detection: `-ep bypass -w hidden -enc` on a command line, a `.bat` spawning `powershell`, and `WebClient.DownloadString` with `IEX` all show up in script-block and process-creation logs.
