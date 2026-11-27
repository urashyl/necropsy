# Community: Discord

The site can link a Discord server and give each daily a discussion thread there once it has closed. Both are optional and off by default.

## Settings

| Setting | | |
| --- | --- | --- |
| `NECROPSY_DISCORD_URL` | optional | An `https://` invite to the server. Shown as **Discord** in the footer of every page, and on closed dailies that have no thread. |
| `NECROPSY_DISCORD_WEBHOOK` | optional, with the next | The webhook URL of a **forum** channel. It holds a secret token: keep it in the environment file (mode 0600), never on a command line or in the repository. |
| `NECROPSY_DISCORD_GUILD_ID` | with the webhook | The server's numeric id, used to build each thread's address. |

Set both webhook settings or neither; the server will not start with one, or with an invite that is not `https://`, or a guild id that is not a number.

To get them: in Discord, create a forum channel (for example `#dailies`). **Edit Channel > Integrations > Webhooks > New Webhook**, then **Copy Webhook URL**. Turn on **Developer Mode** (User Settings > Advanced), right-click the server's icon and **Copy Server ID**. An invite comes from **Invite People**, set to never expire.

## When a daily is discussed

A daily **closes** when its date is over in every timezone: at the end of its date in UTC-12, which is 12:00 UTC the next day. Until then it is someone's puzzle of the day, so the site never points at a discussion of it:

- **Open daily:** no link. A player who has solved it sees "The discussion opens once this daily has closed...".
- **Closed daily:** a link under the answer form, **Discuss it on Discord** to its thread, or, without a thread, **Discuss closed dailies on Discord** to the invite.
- **Intro and Library puzzles:** no link. They are always open.

The site only controls its own links. The server's members can still talk about an open daily anywhere in the server; say in the server's rules that open dailies are not discussed, and moderate it as usual.

## Threads

With the webhook set, the server checks every 5 minutes for dailies that have closed and have no thread, and posts one to the forum channel for each, at most 3 per run:

- Title: `Daily #4: Answer the door`.
- First message: the daily has closed, spoilers are fine, and the puzzle's link. The webhook is told to ping no one.

The thread's address is stored (`discussions` table) and shown on the puzzle page from then on, so each daily gets one thread. Only dailies that closed in the last 14 days get one, so turning the feature on does not post a thread for every old daily.

## When something breaks

| Symptom | Cause, and what to do |
| --- | --- |
| The log says "could not make the discussion thread for ... Discord answered 404" | The webhook was deleted or its URL is wrong. Make a new one and update `NECROPSY_DISCORD_WEBHOOK`. The run stops at the first failure and tries again in 5 minutes. |
| "Discord answered 400" mentioning `thread_name` | The webhook belongs to a text channel, not a forum channel. Threads need a forum. |
| A thread was deleted in Discord | The page still links it. Delete its row (`DELETE FROM discussions WHERE pack_id = '...'`) and a new one is posted on the next run, if the daily closed in the last 14 days. |
| A thread should be made for an older daily | Post it by hand, then add its row: `INSERT INTO discussions (pack_id, url, created_at) VALUES ('<id>', 'https://discord.com/channels/<guild>/<thread>', strftime('%s'))`. Restart to load it. |
| The webhook URL leaked | Delete the webhook in Discord (that revokes the token), make a new one, update the setting, restart. |
