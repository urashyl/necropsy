A **domain generation algorithm** (DGA) gives malware a rendezvous with its operator that cannot be blocked in advance: both sides compute the same name from a shared seed and the date, so there is no fixed address to take down. Conficker, Necurs and many ransomware families have used one.

The weakness is the same property. The algorithm is deterministic, so once an analyst has the code, they can compute tomorrow's, next month's and last year's domains, register them, and watch every infected machine knock on the door. That is a **sinkhole**, and it is how botnets get counted and how takedowns are scoped.

This DGA is a linear congruential generator seeded with the date. The details that trip people up are masking to 31 bits (`& 0x7FFFFFFF`, as C code would overflow), using the high bits (`>> 16`), because the low bits of an LCG cycle quickly, and picking the TLD from the final state. A port to another language that treats the integers differently produces a different, wrong domain, which is why analysts run the original code rather than re-implementing it from reading.

In a SOC, you look for hosts producing many NXDOMAIN answers for random-looking 12-letter names under rotating cheap TLDs.
