`unpack()` gives `c2=https://cdn-telemetry.mirrorsync.test/v2/ping;sleep=90;jitter=20`, so the host is **cdn-telemetry.mirrorsync.test**.

| Layer | Looks like | Undo with |
| --- | --- | --- |
| Base64 | `A-Z a-z 0-9 + /`, length a multiple of 4, maybe `=` at the end | `base64 -d`, CyberChef From Base64 |
| URL encoding | `%` and two hex digits | `urllib.parse.unquote`, URL Decode |
| Hex | only `0-9 a-f`, even length | `bytes.fromhex`, `xxd -r -p`, From Hex |
| XOR with one byte | unreadable bytes, same length as the plaintext | XOR each byte with the key again |

XOR is its own inverse: `p ^ k ^ k == p`. With a one-byte key there are only 256 possibilities, so even without the key in the code, trying them all and looking for readable text finds it.

Encoding is not encryption. Every layer here is reversible without a secret. The point of stacking them is to get past tools that search samples for URLs.

### What a defender takes from it

The decoded string gives the C2 host and the beacon timing (90 seconds, 20% jitter). Decode once by hand, then put the steps in a script or a saved CyberChef recipe, because the next sample from this family will use the same layers.
