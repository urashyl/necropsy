The answer is `https://cdn.jsdelivr-static.invalid/npm/jquery.ui@1.13.2/core.min.js`.

Most of the file is what it looks like. The cart widget comes from the shop's own static host, in plain text. The part that matters is the "performance helper" at the end.

It only runs when the first part of the path is `checkout`, so it never appears on product pages, and anyone checking the home page sees nothing load. The URL is stored as character codes with 8 (the length of `checkout`) added to each, so converting the numbers as they are gives you nonsense, and searching the file for `http` only finds the cart widget.

Take the 8 off and it reads normally:

```bash
python3 -c 'print("".join(chr(c - 8) for c in [112,124,124,120,123,66,55,55]))'   # paste the whole list
```

In the browser console, `t.map(c => String.fromCharCode(c - 8)).join("")` with `t` set to the array does the same. Copy just the array, not the line that calls `load`.

The host is made to look like jsDelivr and the path like a jQuery UI release, which is what a developer glancing at the network tab expects to see. But the checkout check is the clever part. That's the one page with a card form on it. Card skimmers on shops have worked this way for years: a short loader hidden in a script the site already trusts, which fetches the real skimmer only on the page where there's something to steal.
