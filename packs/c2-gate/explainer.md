The server was fussy on purpose. It only answered a request that matched the implant in every detail: the exact `User-Agent`, a 16-digit hex `X-Client-Id`, and a `d=` form field holding base64 of four values joined by `|`. Everything else got a stock nginx 404.

That is called **cloaking**, and real command servers do it for the same reason: scanners, crawlers and curious analysts see nothing worth reporting. The tell is that the protocol is stricter than any browser would ever be. The implant's own strings (a format string for the `User-Agent`, the two paths, the header name, the `%s|%s|%s|%s` frame) were enough to rebuild a valid request.

The check-in then returned a one-time path, and only a client that had checked in could use it. Following it gave the tasking, which here carried the key for the next stage.

Rebuilding a server like this is a standard step. It lets you reproduce what the malware does next without letting it talk to the real attacker. Tools such as INetSim and FakeNet-NG do the generic version of this, and a capture like the one in this puzzle is what you write a faithful version from.
