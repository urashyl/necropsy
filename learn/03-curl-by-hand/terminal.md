## Install

curl is already installed on macOS, Windows 10 and later (`curl.exe`), and nearly every Linux. Check with `curl --version`. If not: `sudo apt install curl` or `sudo dnf install curl`.

On Windows, use `curl.exe` in PowerShell (plain `curl` there is an alias for `Invoke-WebRequest`), and double quotes instead of single quotes: `curl.exe -H "X-Operator: nightjar" ...`. Or use Git Bash or WSL, where the commands below work as written.

## Walkthrough

Sign in, open the practice puzzle and press **Spin up instance**. Copy its URL into a variable so the commands stay short:

```bash
I=https://<your instance>
curl -i "$I/"
curl -i "$I/notes?id=3"
curl -i -X POST "$I/panel/login"
curl -i -X POST -H 'X-Operator: nightjar' "$I/panel/login"
curl -i -H 'X-Operator: nightjar' --data 'user=op&pin=<pin>' "$I/panel/login"
```

Read each response before the next request: the status and the message say what to change. Add `-v` to one of them to see the request curl sent, lines starting with `>`.

The instance only answers your requests while it runs; when its time is up, start it again.
