A build server ran a binary named `svcupd` out of `/var/tmp` and it was gone by morning. EDR kept a copy. All the network team saw was lookups for a name that never resolved. We rebuilt the operator's gateway as it was when we took it down: it is a real server running in an isolated container, started just for you. Start it below.

The binary is a real, stripped 64-bit ELF, and the details of where it was going are not in the clear: no domain, no path, no credentials in the strings. They are in the file, encrypted. Recover them, either by reading the binary or by watching it decrypt them, and use them against the gateway as the beacon would. The gateway holds the next key the operator rotates to. Submit it as the flag.

The binary is harmless: it decrypts its configuration and tries to resolve a name that cannot exist. Still, run it only somewhere you can throw away. The values are different for every player, and nothing here can reach the internet.
