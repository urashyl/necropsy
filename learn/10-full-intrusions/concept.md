A 5/5 puzzle has no new technique. It chains the ones from the earlier modules, and leaves out the steps a guided puzzle would give you.

A typical one: a packed sample, a capture of it talking, and a live server that is the real C2 program in a sandbox. The path to the flag goes through:

1. **Triage** (module 6): what is the file, what strings and markers does it have, what is packed.
2. **Unpacking** (modules 4 and 6): peel the layers until the real code or config is readable. For a script, replace the `exec` or `eval` with `print`. For a binary, find the config blob and the routine that decodes it.
3. **Protocol** (modules 2 and 9): from the capture and the code, write down every message: path, headers, body format, encryption.
4. **Crypto** (module 5): find where each key comes from. Hard-coded, derived, or sent by the server.
5. **Reimplementation** (module 7): write a client that speaks the protocol. Often the fastest way is to reuse the sample's own functions: copy its decryption routine instead of rewriting it.
6. **Talk to the server** (module 9): check in, follow it, get the flag.

## What makes it hard

- **Real servers keep state.** A session, a counter, a key per check-in. Requests in the wrong order, or replayed, fail.
- **Replay protection.** A nonce or timestamp in each message means a captured request can't be sent again as is: you have to build fresh ones.
- **Anti-analysis.** Checks for a debugger, a sandbox, a renamed file, or a time window, that change what the sample does.
- **Less guidance.** The brief says what to find, not how.

## Habits that help

- Keep notes: every value, where it came from, what it is for.
- Reproduce a captured exchange first, with your own code, before talking to the server: if you can decrypt the capture, your crypto is right.
- When a request fails, compare it with a known-good one, byte by byte.
- Change one thing at a time.

Each practice puzzle here is from the Library, where the hardest puzzles live. The container labs run real C2 code and need the site's range; if they are missing, the site has not turned it on.
