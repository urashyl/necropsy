Phishing kits are sold and reused, and they are usually sloppier than the people running them. This one left three doors open:

1. `robots.txt` listed `/panel/` and `/backup/`, which is a signpost to exactly the places the author wanted hidden.
2. `/backup/` had directory listing on, and inside it a `.bak` copy of the kit's configuration. A `.bak` suffix stops the server from running the file as PHP, so it is served as plain text and the panel password (and the bot token for exfiltration) is sitting in it.
3. The panel used HTTP Basic authentication with that same password.

`harvest.php.bak` is the other half of the story. It is a typical credential logger: it appends whatever the victim typed to a file, adds the address and browser, and forwards a copy to a chat bot. The `t.js` script on the lure page is the classic cookie logger, a single line that sends `document.cookie` to the attacker. Both are small and old, and that is the point: most of the work in phishing is the lure, not the code.

Defenders use exactly this kind of exposure. Finding the panel and the bot token tells takedown teams where the stolen data goes, and the licence key links this site to every other site built from the same kit.
