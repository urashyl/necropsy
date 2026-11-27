The workspace's **Console** tab sends curl commands to your own instance. Sign in, open the practice puzzle's workspace and press **Spin up instance**.

### With blocks

1. Tick **Block mode**. The command starts as `curl https://<your instance>` and a box for the path.
2. Type `/` in the path box and press **Send**. The **Terminal output** shows what curl would print; **Request sent** and **Response received** show every byte that went over the wire.
3. Change the path to `/notes`. Press **Add query parameter** and type `id=3`. Watch the command under the blocks change: the parameter is added to the URL after `?`.
4. Send it and read the note. It names a path, a header and a form body.
5. Change the path to the login path, **Add method (-X)** with `POST`, and send. Read the status and the message: it says what is missing.
6. **Add header (-H)** with `X-Operator: nightjar`, send, and read the next message. Then **Add body (--data)** with the form the note gives.

### Typed

7. Untick **Block mode**. The command stays, as text. From here on, edit the text: add `-i` to see headers, change a value, send again. **Ctrl+Enter** sends.
8. A flag the console does not support gets an error that names it: try `-A`. It says to use `-H 'User-Agent: ...'` instead.

The checkpoint asks you to type commands, without blocks.
