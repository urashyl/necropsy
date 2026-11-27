Difficulty 5 is done on your own computer, with the tools from module 8: Wireshark and tshark, curl, Python with requests and cryptography, and a hex viewer.

A way to start any 5/5 puzzle:

1. Read the brief and list what you are given: sample, capture, strings, a live server.
2. Triage the sample (module 6): `file`, `strings`, `xxd | head`. Note markers, imports, URLs.
3. If it is a script, unpack it by replacing the final `exec`, `eval` or `Invoke-Expression` with `print`, and decode until it is readable.
4. Open the capture (module 1) and write down every request and reply.
5. Find each key and how it is used (module 5). Test it by decrypting a message from the capture.
6. Write the client in Python (module 7), reusing the sample's own functions where you can.
7. Talk to the instance (module 9).

Start with **Locked out**, which needs no range, then the container labs.
