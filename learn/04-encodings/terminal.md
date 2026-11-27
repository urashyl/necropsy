Each encoding has a one-line undo in a shell or in Python. Python 3 comes with macOS and Linux; on Windows install it from python.org or the Microsoft Store.

```bash
echo 'bmVjcm9wc3k=' | base64 -d          # base64 (macOS: base64 -D on old versions)
echo '6332' | xxd -r -p                    # hex
python3 -c 'import urllib.parse,sys; print(urllib.parse.unquote(sys.argv[1]))' '%2Fgate.php%3Fid%3D7'
```

XOR, and the practice puzzle's four layers, in Python:

```python
import base64, urllib.parse

blob = "JTM1NCUz..."          # CFG's lines joined into one string
a = base64.b64decode(blob)            # layer 1: base64
b = urllib.parse.unquote(a.decode())  # layer 2: URL encoding
c = bytes.fromhex(b)                  # layer 3: hex
d = bytes(x ^ 0x37 for x in c)        # layer 4: XOR with the key
print(d.decode())
```

With an unknown single-byte key, try them all:

```python
for k in range(256):
    out = bytes(x ^ k for x in c)
    if out.isascii() and out.decode(errors="ignore").isprintable():
        print(hex(k), out)
```

Save it as `unpack.py` and run `python3 unpack.py`.
