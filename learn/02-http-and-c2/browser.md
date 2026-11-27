1. Open the practice puzzle's workspace. The **Pcap** tab opens the capture.
2. Filter `http.request`. Two kinds of request are mixed together: a couple of page loads, and a series of POSTs to one host.
3. Filter `http.request.method == "POST"`. Look at the Time column: the gaps are the beacon interval.
4. Click one POST and open the HTTP part of its tree. Note the `User-Agent`, the path and the form body (`id=...&st=...`).
5. Filter `http.response`. Most responses have the same small length. One is longer: click it and press **Follow HTTP stream** to read the whole exchange. That is the tasking.
6. From the stream view, **Send stream to decoder** puts the text in the **Decoder** tab, in case it needs decoding.
7. The **Terminal** line shows the equivalent tshark command for each view.

Now solve the practice puzzle: find the task the server gave the implant.
