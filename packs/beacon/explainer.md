The answer is `static.jquery-cdn.invalid`.

Three things in this log poll on a schedule. The office suite posts telemetry every 300 seconds exactly, the antivirus fetches definitions every hour, and the chat client reconnects its websocket now and then. They all use their own user agents and talk to their vendors, which is what you'd expect.

`static.jquery-cdn.invalid` is different. The gaps between requests are anywhere from about 3¾ to 6¼ minutes: five minutes with 25% jitter, so it doesn't stand out as a perfect interval. Every request fetches the same file, and almost every response is exactly 5,543 bytes. No page ever refers to it: the user's real jQuery comes from `code.jquery.invalid`, with a `Referer` from the news site. The user agent is Internet Explorer 11 on Windows 8.1, while everything the user did themselves came from Edge. And it carries on straight through lunch, when the user wasn't browsing at all.

Twice the response is a different size, and a few seconds later the same host gets a `POST` with several kilobytes of data. That's the implant picking up a task and sending back the result.

To find this kind of thing, sort by host and look at the gaps:

```bash
grep 'GET https://static.jquery-cdn' proxy.log | awk '{print $1}' | sed 's/.*T//;s/Z//' \
  | awk -F: '{s=$1*3600+$2*60+$3; if (p) print s-p; p=s}' | sort -n | uniq -c
```

The paths are a giveaway once you've seen them before. `GET /jquery-3.3.1.min.js` and `POST /jquery-3.3.2.min.js?__cfduid=` come from a jQuery profile for Cobalt Strike that's been public for years, and plenty of intrusions use it unchanged. Disguising the traffic as a popular library makes it look ordinary in a list of URLs. It doesn't help once you look at the timing.
