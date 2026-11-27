Encoding changes how bytes are written down, not who can read them. Anyone can undo it without a key. Malware uses it to get binary data through text-only channels (HTTP, JSON, a script) and to hide strings from tools that search for URLs.

## Base64

Writes any bytes with 64 safe characters: `A-Z a-z 0-9 + /`, and `=` as padding at the end. Every 3 bytes become 4 characters.

- `necropsy` → `bmVjcm9wc3k=`
- Recognize it: only those characters, length a multiple of 4, often ending in `=` or `==`.
- Variants: URL-safe base64 uses `-` and `_` instead of `+` and `/`. PowerShell's `-enc` is base64 of UTF-16LE text, so every other byte of the decoded text is `00`.

## Hex

Each byte as two characters `0-9 a-f`. `c2` → `6332`.

- Recognize it: only hex digits, even length. Sometimes with `\x` before each byte (`\x63\x32`) or spaces between.

## URL encoding (percent-encoding)

A byte that is not allowed in a URL is written as `%` and two hex digits: space is `%20`, `/` is `%2F`, `?` is `%3F`. In form bodies a space can also be `+`.

- Recognize it: `%` followed by two hex digits.

## XOR

Each byte is combined with a key byte: where the bits differ, the result has a 1.

```text
  0100 0001   0x41 'A'
^ 0010 0000   0x20 key
= 0110 0001   0x61 'a'
```

XOR twice with the same key gives back the original: `p ^ k ^ k == p`. So encrypting and decrypting are the same operation.

- **Single-byte key:** every byte XORed with the same value. Only 256 keys exist: try them all and keep the readable result.
- **Repeating key:** byte *i* XORed with `key[i % len(key)]`. If the plaintext has a known part (like `http://`), XOR it with the ciphertext there to get the key.
- Recognize it: unreadable bytes the same length as what they hide, often found next to the key in the code.

## Layers

Malware stacks them: XOR, then hex, then base64. Undo them in reverse order, one at a time, and look at the result after each step:

| After a step you see | Next |
| --- | --- |
| `A-Za-z0-9+/=` | base64 |
| `%xx` | URL decode |
| `0-9a-f`, even length | hex |
| `1f 8b` or `78 9c` at the start | gzip or zlib: inflate it |
| Unreadable, same length | XOR, look for a key |
| Readable text | done |
