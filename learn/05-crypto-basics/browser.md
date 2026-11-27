1. Open the practice puzzle's workspace. `config.json` is in **Files**; it is short, so read it on the puzzle page or send it to the decoder.
2. The fields tell you the cipher (`aes-256-cbc`), how the key is made (`sha256(campaign)`), the IV and the base64 ciphertext.
3. **Derive the key.** In the **Decoder**, put `OPAL-17` in the input and use the operation `SHA2` with size 256. The output is 64 hex digits: 32 bytes, an AES-256 key. Copy it.
4. **Decrypt.** Clear the recipe. Put the `data` value in the input. Add `From Base64`, then `AES Decrypt`. In AES Decrypt set **Key** to the hash (type Hex), **IV** to the `iv` value (type Hex), **Mode** CBC, **Input** Raw, **Output** Raw.
5. The output is JSON. If you see an error about padding, the key or IV is wrong: check that both are set to Hex, not UTF8.
6. The `mac` field is an HMAC. To check it, use `HMAC` with the same key (Hex) and SHA256 on the IV bytes followed by the ciphertext bytes. It is not needed to decrypt.

The same in the **Notebook**, which has Python's `hashlib`:

```python
import hashlib
print(hashlib.sha256(b"OPAL-17").hexdigest())
```

Pyodide's standard library has no AES, so use the decoder for that step, or write the decryption on your own computer (the terminal path shows how).
