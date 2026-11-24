The answer is `https://static.cdn-assets.invalid/lib/jquery.min.js`.

The address is stored as a list of character codes, and the loop turns each number into a letter with `String.fromCharCode` before assigning the result to `script.src`. It hides the URL from a search for `http` and from a quick read, nothing more.

You never need to run the page to undo it. In a browser's developer console (which only evaluates what you type) or any language:

```javascript
String.fromCharCode(104,116,116,112,115,58,47,47,115,116,97,116,105,99,46,99,100,110,45,97,115,115,101,116,115,46,105,110,118,97,108,105,100,47,108,105,98,47,106,113,117,101,114,121,46,109,105,110,46,106,115)
```

```bash
python3 -c 'print("".join(map(chr, [104,116,116,112,115,58,47,47,115,116,97,116,105,99,46,99,100,110,45,97,115,115,101,116,115,46,105,110,118,97,108,105,100,47,108,105,98,47,106,113,117,101,114,121,46,109,105,110,46,106,115])))'
```

The pattern to remember is the **shape**: a list of numbers, a loop, and a `createElement("script")` at the end. That is how a page loads code from somewhere other than where it was served. The fix for an analyst is the same every time: replace the part that runs with a part that prints, and read the result.

A site that loads a script like this hands control of its pages to that host, which is how card skimmers get onto shops.
