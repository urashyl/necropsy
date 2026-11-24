The answer is `sess_9f2c41ab77e0`.

On Linux, Chromium encrypts cookie values with a key it gets from the system keyring. When no keyring is available it falls back to a hardcoded password, `peanuts`, so anyone with the profile directory can decrypt the cookies. The `v10` prefix marks this scheme.

The key and IV are derived like this:

- Key: PBKDF2-HMAC-SHA1, password `peanuts`, salt `saltysalt`, 1 iteration, 16 bytes
- Cipher: AES-128-CBC with PKCS#7 padding
- IV: 16 space characters (`0x20`)

With OpenSSL:

```bash
KEY=$(openssl kdf -keylen 16 -kdfopt digest:SHA1 -kdfopt pass:peanuts \
  -kdfopt salt:saltysalt -kdfopt iter:1 -binary PBKDF2 | xxd -p)
echo 'a1b29fda7ee8756b748239a07fc2659ef5e624fabb5ad9017fb9822c91129481' \
  | xxd -r -p \
  | openssl enc -d -aes-128-cbc -K "$KEY" -iv 20202020202020202020202020202020
```

The leading `763130` in the dump is `v10` in hex and has to be removed first.

This matters for incident response in both directions. A copied profile from a machine without a keyring gives up its sessions, and so does an attacker's copy of yours. Newer Chromium versions also prepend a hash of the cookie's host to the plaintext, so the decrypted value on current builds has 32 extra bytes at the start.
