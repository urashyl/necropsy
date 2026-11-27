A **packet capture** (pcap) is a recording of network traffic: every packet a machine sent or received while the capture ran, with the time it was seen. Malware has to talk to someone, so a capture is often the clearest record of what it did.

## Layers

Each packet is wrapped several times, and a capture viewer shows every layer:

| Layer | Answers | Example fields |
| --- | --- | --- |
| Ethernet | which network card, on the local network | `eth.src` |
| IP | which machine, anywhere | `ip.src`, `ip.dst` |
| TCP or UDP | which program on that machine (the port) | `tcp.port`, `udp.port` |
| Application | what was actually said | `http.host`, `http.request.uri`, `dns.qry.name` |

The puzzles here are about the top layer: HTTP. The layers under it say who talked to whom.

## Conversations, not packets

A web request is not one packet. It is a **TCP conversation**:

1. Three packets open the connection (SYN, SYN-ACK, ACK): the *handshake*.
2. The client sends the request. A large one is split across several packets.
3. The server sends the response, also split up.
4. A few packets close the connection (FIN).

Before that, the client usually asks DNS for the server's address: a UDP query and its answer.

Reading packets one by one is slow. **Follow stream** puts a conversation's bytes back together in order and shows them as text: the request, then the response, as the two programs saw them.

## Filters

A capture of a few minutes can hold thousands of packets. A **display filter** hides everything that does not match. Display filters name a field and a value:

- `http.request`: packets that carry an HTTP request.
- `http.response.code == 404`: responses with status 404.
- `ip.addr == 192.0.2.77`: anything to or from that address.
- `tcp.stream == 3`: the fourth TCP conversation (numbering starts at 0).
- `dns.qry.name contains "share"`: DNS queries for names with "share" in them.

Combine with `&&` (and), `||` (or) and `!` (not).

## An HTTP request

```http
GET /s/8fk2Lq/invoice_0921.zip HTTP/1.1
Host: dl.quickdocs-share.test
User-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:128.0) ...
Referer: http://mail.corp-portal.test/inbox?folder=new
```

The first line is the **method**, the **path** and the version. Then headers, one per line: `Host` is the server's name, `User-Agent` the program that sent it, `Referer` the page the user came from. A blank line ends the headers; a body, if there is one, follows.
