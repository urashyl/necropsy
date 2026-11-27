Encoding hides nothing from someone who knows the scheme. **Encryption** does: without the key, the ciphertext is useless. So with encrypted malware data, the question is always "where does the key come from?" The answer is usually "the sample itself", because the malware has to decrypt its own data.

## Words

| Term | Meaning |
| --- | --- |
| Plaintext | The readable data. |
| Ciphertext | The encrypted data. Looks random. Usually stored as base64 or hex. |
| Key | The secret that turns one into the other. |
| Symmetric | The same key encrypts and decrypts. AES, RC4, XOR. |
| Asymmetric | A public key encrypts, a private key decrypts. RSA, X25519. Malware uses it so a captured sample cannot decrypt what it sent. |

## AES

The standard symmetric cipher. A block cipher: it encrypts 16 bytes at a time. The key is 16, 24 or 32 bytes (AES-128, 192, 256).

The **mode** says how blocks are chained:

- **ECB**: each block alone. Identical plaintext blocks give identical ciphertext: a pattern you can see. Avoid.
- **CBC**: each block is mixed with the previous ciphertext block. The first is mixed with an **IV** (initialization vector), 16 bytes that are not secret and are stored with the ciphertext. Plaintext is padded to a multiple of 16 (PKCS#7: the last byte says how many padding bytes there are).
- **GCM**, **CTR**: modes with a **nonce** instead of an IV. GCM also adds an authentication tag.

To decrypt AES you need: the key, the mode, the IV or nonce, and the ciphertext. Try **Raw** output first; if the result has 1 to 16 identical bytes at the end, that was padding.

## Hashes and HMAC

A **hash** (SHA-256, MD5) turns any input into a fixed-size fingerprint. It cannot be undone. Same input, same hash.

An **HMAC** is a hash mixed with a key. It proves the data came from someone with the key and was not changed. It does not hide the data: **an HMAC is not encryption**, and it does not help you decrypt. It matters when you want to change the data (forge a config, a C2 message) and have the receiver accept it: then you must compute the HMAC too.

## Deriving a key

Malware rarely stores the key as plain bytes. It **derives** it: computes it from something else, at runtime.

- A hash of a string in the sample: `key = sha256("OPAL-17")`.
- A hash of something on the victim's machine: the computer name, a volume serial. Then the same sample has a different key on each machine.
- A real key-derivation function (PBKDF2, scrypt) from a password, with a salt and many rounds.
- A key sent by the C2 at check-in.

Find the code that makes the key, repeat it, and you have the key.
