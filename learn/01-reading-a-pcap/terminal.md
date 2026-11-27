## Install

| System | Command |
| --- | --- |
| Debian, Ubuntu | `sudo apt install wireshark tshark` (say yes to "non-superusers capture") |
| Fedora | `sudo dnf install wireshark wireshark-cli` |
| macOS | `brew install --cask wireshark` (tshark is included) |
| Windows | Installer from wireshark.org; tshark is in `C:\Program Files\Wireshark\` |

Check it: `tshark --version`.

## Walkthrough

Download `capture.pcap` from the puzzle page.

1. Open it in Wireshark: `wireshark capture.pcap`. The packet list, the protocol tree and the bytes pane are the three parts of the window.
2. In the filter bar type `http.request` and press Enter. Only the requests are left.
3. Select one. The middle pane is the protocol tree: open **Hypertext Transfer Protocol** to see the headers. Right-click a field, **Apply as Filter**, to filter on it.
4. Right-click the request, **Follow > HTTP Stream**. The conversation opens as text, client in red, server in blue.

The same in tshark, which you will use more as the puzzles get harder:

```bash
tshark -r capture.pcap                       # every packet, one line each
tshark -r capture.pcap -Y http.request       # only the requests
tshark -r capture.pcap -Y http.request -T fields -e http.host -e http.request.uri
tshark -r capture.pcap -Y dns -T fields -e dns.qry.name -e dns.a
tshark -r capture.pcap -q -z follow,http,ascii,3   # conversation 3, as text
```

`-Y` takes the same display filters as Wireshark. `-T fields -e` prints only the fields you name, one line per packet, which is easy to read and easy to script.

Now solve the practice puzzle: find the request that downloaded a zip file, and the host it went to.
