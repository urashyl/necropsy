You need nothing installed: the workspace runs Wireshark's own dissectors in your browser.

1. Open the practice puzzle, then **Open the workspace**. The **Pcap** tab loads Wireshark once (about 20 MB) and opens `capture.pcap`.
2. The packet list has Wireshark's columns: number, time, source, destination, protocol, length, info. Scroll it: DNS lookups, TCP handshakes, HTTP.
3. Type `http.request` in **Display filter** and press **Apply**. Only the requests are left, with their method and path in the Info column.
4. Under the filter, the **Terminal** line shows the tshark command that gives the same list. You will use it in module 8.
5. Click a request. Its protocol tree opens below: Frame, Ethernet II, Internet Protocol, Transmission Control Protocol, Hypertext Transfer Protocol. Open the HTTP part to see each header.
6. Every field in the tree has **Filter** next to it, which makes a filter from that field. Try it on the `Host` line.
7. Press **Follow HTTP stream**. The request and the response appear as text, the way the two programs saw them. **Follow TCP stream** shows the raw bytes of the same conversation.
8. Clear the filter and filter on `dns`. Each query has an answer with the address the name resolved to.

Now solve the practice puzzle: find the request that downloaded a zip file, and the host it went to.
