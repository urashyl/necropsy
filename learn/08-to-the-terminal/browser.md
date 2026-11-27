This module is the same on both paths: it moves you to the terminal. Follow the install steps in the concept above for your system, then redo two puzzles you already solved in the browser.

## Module 1 again, with tshark

1. On the puzzle page of **First capture**, download `capture.pcap` (the **Raw** link, or **Files** in the workspace).
2. Open a terminal in the folder you saved it to.
3. Run the command from the workspace's pcap viewer Terminal line: `tshark -r capture.pcap -Y http.request`.
4. Get the host and path of each request: `tshark -r capture.pcap -Y http.request -T fields -e http.host -e http.request.uri`.
5. Open the same file in Wireshark, filter `http.request`, and **Follow > HTTP Stream** on the download.

## Module 3 again, with curl

1. Start the **Knock knock** instance.
2. Run the same commands you sent from the console, with `curl -i` in your terminal. The console's **Request sent** and `curl -v` show the same request.
3. Then the same in Python with `requests`, as in module 7.

When both work, you have everything the tier 4 and 5 puzzles need.
