Office macros are the classic way in. This one does four things that are worth knowing.

**It hides the URL in plain sight.** `hxxp` is the convention analysts use to defang links, and the macro puts `http` back with `Replace`. A string search for `http://` finds nothing in the source. The per-victim path is hex, decoded two characters at a time.

**It fingerprints the machine.** It exits when `UserName` is empty and when the domain is `WORKGROUP`, which is how most analysis sandboxes look. Skipping the check on a real victim costs nothing, so sandboxes are the only thing it avoids.

**It authenticates itself to its own server.** The `User-Agent` copies what Word really sends, and `X-Build` carries `1337 Xor 42`. Anyone who requests the URL without both gets a 404, so a scanner sees nothing. (The XOR is computed at run time, so a rule that greps for the final number never matches.)

**It writes the answer to `%TEMP%`.** A downloaded second stage is saved and normally launched afterwards.

Defence: block macros from the internet (Mark-of-the-Web), and alert on `WINWORD.EXE` making outbound web requests or spawning children.
