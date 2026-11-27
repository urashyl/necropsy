The workspace covers difficulty 1 to 3. Harder puzzles need what a browser cannot do: run a real binary, use a large Python library, handle a capture of hundreds of megabytes, or keep a script running for an hour. From tier 4 on, you work on your own computer.

Nothing you learned changes. Each workspace tool has showed its terminal equivalent all along:

| Workspace | Your computer | Install |
| --- | --- | --- |
| Pcap viewer | Wireshark, `tshark` | wireshark.org, or your package manager |
| Request console | `curl` | already installed on most systems |
| Notebook | `python3` with `requests` | python.org; `pip install requests` |
| Decoder | CyberChef (any browser), or `base64`, `xxd`, Python | gchq.github.io/CyberChef, or nothing |
| Hex viewer | `xxd`, `strings`, `file` | included in Linux and macOS |

## A safe place to work

Puzzle files are real malware samples, given harmless names (`.txt`, `.sample`) so they never run by accident. Keep it that way:

- Work in one folder, such as `~/necropsy`.
- Read samples; do not run them. When a puzzle needs a binary run, it says so, and you do it in a virtual machine with no network, not on your own system.
- Your antivirus may flag samples. That is expected: they are malware. Excluding one folder is safer than turning protection off.

## Installing

**Linux (Debian, Ubuntu):**

```bash
sudo apt install wireshark tshark curl python3 python3-pip python3-requests python3-cryptography xxd binutils
sudo usermod -aG wireshark "$USER"     # capture without root; log out and in again
```

**macOS:** install Homebrew from brew.sh, then:

```bash
brew install --cask wireshark
brew install python
python3 -m pip install requests cryptography
```

curl, xxd and strings are already there.

**Windows:** install Wireshark (tshark is included) from wireshark.org and Python from python.org (tick **Add python.exe to PATH**), then in PowerShell:

```powershell
py -m pip install requests cryptography
curl.exe --version
```

In PowerShell, `curl` is an alias for `Invoke-WebRequest`; always type `curl.exe`. WSL (`wsl --install`) gives you a full Linux shell where every command in these modules works as written.

## Check it works

```bash
tshark --version
curl --version
python3 -c "import requests, cryptography; print('ok')"
```
