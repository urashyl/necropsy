A warehouse manager double-clicked a "shipment notice", and the script that opened was not a document. We saved it as `Shipment_Notice.vbs.txt` (renamed so it cannot run by accident) and rebuilt the server it contacts on your instance. Start it below.

The script reaches out to `cdn.shipnotice.invalid`. That name does not exist anywhere, so send what the script would send to your instance's link instead. Read how it builds its request, send exactly that, and read what comes back. The second stage holds a key. Submit it as the flag.
