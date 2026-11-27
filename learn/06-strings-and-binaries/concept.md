Triage is the first look at a sample: what is it, what does it talk to, and is it worth a deeper look. Most of it can be done without running anything, by reading the file's bytes.

## Strings

`strings` lists every run of printable characters at least *n* long. In a binary that is noise mixed with gold:

- **Imports** (`LoadLibraryA`, `GetProcAddress`, `WinHttpSendRequest`, `VirtualAlloc`): what the code does. Networking, memory allocation for injected code, persistence.
- **URLs, hosts, paths, user agents**: where it talks.
- **Registry keys** (`SOFTWARE\Microsoft\Windows\CurrentVersion\Run`): how it survives a reboot.
- **Format strings** (`%s|%s|%s`, `%016llx`): the shape of what it sends.
- **Markers**: short fixed strings before embedded data, like `PRF1:` or `STRTABLE`.

Malware authors know this, so the interesting strings are often hidden. What is left in the clear (imports, markers, format strings) says where to look.

## Magic bytes

Files and formats start with fixed bytes. After every decoding step, look at the first few:

| Bytes | Format |
| --- | --- |
| `4d 5a` (`MZ`) | Windows executable or DLL |
| `7f 45 4c 46` (`.ELF`) | Linux executable |
| `50 4b 03 04` (`PK`) | zip (also docx, xlsx, jar, apk) |
| `1f 8b` | gzip |
| `78 9c`, `78 da`, `78 01` | zlib |
| `89 50 4e 47` (`.PNG`) | PNG image |
| `25 50 44 46` (`%PDF`) | PDF |
| `{` or `[` | JSON |

## String tables

A common way to hide strings: put them all in one table, each one XORed (or otherwise encoded), and decode them at runtime by index. The table usually has a small header: a marker, a key, a count, lengths or terminators. Find the table by its marker or by a run of high-entropy bytes near the code that reads it, work out the layout from a few bytes, and decode all of it at once.

## Embedded blobs

Configuration (C2 hosts, sleep, user agent, keys) is often one blob: compressed, encoded, maybe encrypted. Typical layers are base64 → zlib → JSON, or XOR → base64. Find it by its marker or its alphabet, then peel it with the steps from module 4.

## Offsets

A position in the file, counted from 0, usually in hex. "The profile is at 0x7a2" lets anyone jump to it, and lets a script read it directly.
