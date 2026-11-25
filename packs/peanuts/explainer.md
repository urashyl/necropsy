The answer is `sess_9f2c41ab77e0`.

On Linux, Chromium encrypts cookie values with a key it stores in the system keyring, GNOME's libsecret or KDE's KWallet. This kiosk had neither, so Chromium did what it always does in that case and used the password built into it, `peanuts`. Values encrypted that way start with `v10`. Values protected by a keyring key start with `v11`.

The rest is in Chromium's source. The key is PBKDF2-HMAC-SHA1 of `peanuts` with the salt `saltysalt`, one iteration, 16 bytes. The cipher is AES-128-CBC, and the IV is 16 spaces. Drop the `v10` and decrypt:

```bash
KEY=$(openssl kdf -keylen 16 -kdfopt digest:SHA1 -kdfopt pass:peanuts \
  -kdfopt salt:saltysalt -kdfopt iter:1 -binary PBKDF2 | xxd -p)
echo '<the hex after 763130>' | xxd -r -p \
  | openssl enc -d -aes-128-cbc -K "$KEY" -iv 20202020202020202020202020202020 \
  | tail -c +33
```

The `tail -c +33` is the second catch. From cookie database version 24, which the meta table shows, Chromium puts the SHA-256 of the cookie's `host_key` in front of the value before encrypting it. That stops someone copying an encrypted value into another site's row. It also lets you check your work: the first 32 bytes should be `printf %s .example-shop.invalid | sha256sum`.

You may have noticed that the three `.example-shop.invalid` rows all start with the same 64 hex digits after `v10`. That's the fixed IV. With the same key, the same IV and the same first 32 bytes of plaintext (the host hash), you get the same first two blocks of ciphertext. It gives nothing away here, but it shows what this is. Without a keyring, Chromium on Linux is hiding cookies, not protecting them.

That cuts both ways in an investigation. A copy of a profile like this gives you every session in it. If an attacker copied yours, it gives them the same.
