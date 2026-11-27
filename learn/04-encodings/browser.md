The **Decoder** tab is CyberChef, running in your browser.

1. Open the practice puzzle's workspace. In **Files**, press **Send to the decoder** next to `dropper.py.txt`. The whole file lands in CyberChef's **Input**.
2. Delete everything in the input except the base64 string: join the lines of `CFG` into one, without the quotes.
3. In **Operations** search for `From Base64` and drag it into **Recipe**. The **Output** shows the result: `%`-escapes.
4. Add `URL Decode`. The output is now hex digits.
5. Add `From Hex`. Now it is unreadable bytes: XOR.
6. Add `XOR`, set the key to `37` and its type to **Hex** (the code says `K = 0x37`). The output is the configuration.
7. CyberChef keeps the recipe in its URL, so a recipe can be saved and shared. Hints in these puzzles can open the decoder with a recipe filled in: look for **Open in the decoder**.

The **Terminal** line under the decoder shows the shell equivalent of each step.

If you get stuck on a layer, **Magic** (search for it in Operations) tries common decodings and shows which give readable output.
