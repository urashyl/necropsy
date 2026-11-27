Download `capture.pcap` from the puzzle page. You need Wireshark and tshark from module 1.

```bash
# every request: time, host, method, path, user agent
tshark -r capture.pcap -Y http.request -T fields \
  -e frame.time_relative -e http.host -e http.request.method -e http.request.uri -e http.user_agent

# only the POSTs, with the time since the previous one shown
tshark -r capture.pcap -Y 'http.request.method == "POST"' -T fields \
  -e frame.time_delta_displayed -e http.file_data

# response sizes: the odd one out is the tasking
tshark -r capture.pcap -Y http.response -T fields -e tcp.stream -e http.content_length -e http.content_type

# read that conversation
tshark -r capture.pcap -q -z follow,http,ascii,<stream number>
```

In Wireshark: filter `http.request.method == "POST"`, then **Statistics > Conversations** shows how many connections went to each host, and **Follow > HTTP Stream** on the odd response shows the tasking.

Now solve the practice puzzle: find the task the server gave the implant.
