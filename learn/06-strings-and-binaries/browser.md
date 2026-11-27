1. Open the practice puzzle's workspace and the **Hex** tab. It opens `implant.bin.sample`.
2. The first line starts with `4d5a`, `MZ`: a Windows executable. A few lines down, `PE`.
3. Switch **View** to **Strings**. Each line is an offset and a string of at least **Min length** characters. Set it to 8 to cut the noise.
4. Note the imports and the two markers: `STRTABLE` and `PRF1:`. The Terminal line shows the `strings` command for the same list.
5. Copy the offset of `PRF1:` into **Offset** and switch back to **Hex**: the view jumps there. The bytes after the marker are base64 characters, up to a `00` byte.
6. Copy the base64 (from the Strings view, without `PRF1:`) into the **Decoder**: `From Base64`. The output starts with `78`: zlib. Add `Zlib Inflate`. The output is JSON with the profile.
7. Back in **Hex**, jump to `STRTABLE`. The next byte is the key, the one after the count, then the XORed strings. In the decoder, `From Hex` on those bytes, then `XOR` with the key, shows the table.

The **Notebook** can do the same in a few lines once you know the layout:

```python
from necropsy import file
import base64, zlib
b = await file("implant.bin.sample")
i = b.index(b"PRF1:") + 5
print(zlib.decompress(base64.b64decode(b[i:b.index(b"\0", i)])))
```
