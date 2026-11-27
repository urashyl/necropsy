Install the `cryptography` package for Python: `python3 -m pip install cryptography` (on Debian or Ubuntu, `sudo apt install python3-cryptography`).

Download `config.json` from the puzzle page and save this as `decrypt.py` next to it:

```python
import base64, hashlib, hmac, json
from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes

cfg = json.load(open("config.json"))
key = hashlib.sha256(cfg["campaign"].encode()).digest()   # derive the key
iv = bytes.fromhex(cfg["iv"])
data = base64.b64decode(cfg["data"])

# The HMAC proves the config was not changed. Checking it is optional.
print("mac ok:", hmac.compare_digest(hmac.new(key, iv + data, hashlib.sha256).hexdigest(), cfg["mac"]))

dec = Cipher(algorithms.AES(key), modes.CBC(iv)).decryptor()
plain = dec.update(data) + dec.finalize()
plain = plain[:-plain[-1]]          # PKCS#7: the last byte is the padding length
print(plain.decode())
```

`python3 decrypt.py`.

With OpenSSL instead (it takes the key and IV as hex):

```bash
KEY=$(printf 'OPAL-17' | sha256sum | cut -d' ' -f1)       # macOS: shasum -a 256
jq -r .data config.json | base64 -d | openssl enc -d -aes-256-cbc -K "$KEY" -iv "$(jq -r .iv config.json)"
```
