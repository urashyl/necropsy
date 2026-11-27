A warehouse manager double-clicked a "shipment notice", and the file that opened was a script. It is `Shipment_Notice.vbs.txt` (renamed so it cannot run). Start an instance: the file is linked on its home page, at `/dl/Shipment_Notice.vbs.txt`.

The script calls `cdn.shipnotice.invalid`, which does not resolve. Work out how it builds its request, send exactly that to your instance, and read the reply. The second stage holds a key. Submit it as the flag.
