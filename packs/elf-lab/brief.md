A build server ran a binary named `svcupd` out of `/var/tmp` and it was gone by morning. EDR kept a copy. The network team saw only lookups for a name that never resolved.

The binary is a stripped 64-bit ELF. No domain, path or credentials appear in its strings: they are in the file, encrypted. Recover them by reading the binary or by watching it decrypt them, then use them against the gateway as the beacon would. The gateway holds the next key the operator rotates to. Submit it as the flag.

Run it only on a machine you can throw away.
