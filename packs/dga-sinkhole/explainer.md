A **domain generation algorithm** (DGA) gives malware a rendezvous with its operator that cannot be blocked in advance: both sides compute the same name from a shared seed and the date, so there is no fixed address to take down. Conficker, Necurs and many ransomware families have used one.

The weakness is the same property. The algorithm is deterministic, so once an analyst has the code, they can compute tomorrow's, next month's and last year's domains, register them, and watch every infected machine knock on the door. That is a **sinkhole**, and it is how botnets get counted and how takedowns are scoped.

This sample makes that harder in three ways that real loaders use.

**The program is not readable.** Every string, including the registry path and the names of the methods it calls, sits in an XOR-encoded table that is rotated at start-up and decoded on use. The routine itself is a `while`/`switch` state machine whose order is a string, `"3|0|4|1|2"`, so reading it top to bottom gives the steps out of order. Neither is hard to undo once seen: run the table's decoder over every entry, and follow the state string.

**It is seeded by the machine and by its own name.** The seed mixes a hash of the victim's `MachineGuid` with a hash of the script's file name, so the same file produces different domains on different machines, and the file renamed to `netcheck.js.txt`, as it is here, produces a decoy. That is an anti-analysis trick: an analyst who runs the sample as found gets a plausible domain that is wrong, and the sinkhole shows it as parked by a security vendor. The inputs are in the incident notes: the GUID, and the path the loader ran from.

**It runs on Windows Script Host.** `WScript` and `ActiveXObject` do not exist outside it, so the file cannot just be run. You supply them: in Node, define `WScript.ScriptName` and an `ActiveXObject` that returns the GUID for the registry read and records the request it would send, and replace `Date` with the date you want. Or re-implement it, in which case the details that trip people up are the 32-bit arithmetic (`Math.imul`, `| 0`, `>>> 0`, a mask to 31 bits), because JavaScript numbers are doubles and a plain multiplication loses the low bits.

In a SOC, you look for hosts producing many NXDOMAIN answers for random-looking 12-letter names under rotating cheap TLDs.
