The answer is `LOVE-LETTER-FOR-YOU.TXT`.

The real file name is `LOVE-LETTER-FOR-YOU.TXT.vbs`. `.vbs` is a known type, so Windows hid it, leaving `LOVE-LETTER-FOR-YOU.TXT`, which looks like a harmless text file. Opening it did not show text: it ran the script. Only the **last** extension counts, and it is the one that decides what happens when you open the file.

The message does very little else, and that was enough. The subject (`ILOVEYOU`) and the one-line body (`kindly check the attached LOVELETTER coming from me.`) work because they arrive from someone you know. The worm sent itself to the people in the infected user's address book, so each message came from a real acquaintance.

What changed afterwards:

- **Show file extensions.** Turning off "hide extensions for known file types" removes the disguise, and it has been a standard hardening step since.
- **Block script attachments.** Mail gateways began refusing `.vbs` and similar types outright. Many still block executable and script extensions today, including inside zip files.
- **A double extension is a flag by itself.** `report.pdf.exe`, `invoice.doc.js` and `photo.jpg.scr` are the same trick.

When you analyse a message like this, read the raw `Content-Type` and `Content-Disposition` headers. They show what the file really is, whatever the mail program displays.

Sources: [F-Secure description of Email-Worm:VBS/LoveLetter](https://www.f-secure.com/v-descs/love) and [Wikipedia: ILOVEYOU](https://en.wikipedia.org/wiki/ILOVEYOU). The original script is not included here, only the artifacts an analyst would work from.
