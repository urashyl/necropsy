# Accounts

Accounts are optional. Anyone can solve puzzles without one, and their progress stays in the browser. A signed-in user's solves are also stored on the server, so progress and streaks follow them to other devices.

Code: `auth.rs` (credentials, tokens, rate limits, extractors, admin bootstrap), `accounts.rs` (user pages), `admin.rs` (admin page), `db.rs` (storage), `mail.rs` (SMTP).

## First run and the admin account

There is no default account. When the database has no admin, the server generates one on startup and prints its username (`admin-` plus six random characters) and a 24-character random password to the log, once:

```text
WARN necropsy_web: first run: created the admin account. These credentials are shown once and cannot be recovered.

    username: admin-k3j9x2
    password: ...
```

Only an Argon2id hash is stored. If the credentials are lost, delete the database to start over, or add a new admin directly in SQLite. Log in at `/login` and change the password at `/account`.

The admin starts with no email address. At `/account` the admin can:

- **Change the username** to anything that fits the format, including names regular users cannot register (`admin`, for example). Regular users cannot change their username.
- **Add an email address** (when SMTP is configured), confirmed by a link like any email change. Once it is set, password reset by email works for the admin too, which is the way back in if the password is lost. Like every address it is never displayed.

The admin account cannot be disabled, deleted, or self-deleted from the UI.

## Trying live puzzles

The admin page lists every live puzzle that is loaded, whether or not it is released, with a button to **start an instance** of it for the admin. It has no captcha and no limit of three, runs the same as a player's, and the page shows its link, the time it has left and **its flag**, so a puzzle can be tried end to end (and its flag checked) without playing it first. Stop ends it at once. A puzzle that needs the range is loaded only when `NECROPSY_RANGE_URL` is set, and the page says when live puzzles are off.

## Email

SMTP is configured only through environment variables:

| Variable | Default | |
| --- | --- | --- |
| `NECROPSY_SMTP_HOST` | unset | Turns SMTP on. |
| `NECROPSY_SMTP_PORT` | 587, or 465 for `tls`, 25 for `none` | |
| `NECROPSY_SMTP_USERNAME`, `NECROPSY_SMTP_PASSWORD` | unset | Both or neither. |
| `NECROPSY_SMTP_FROM` | none, required with a host | For example `necropsy <no-reply@example.com>`. |
| `NECROPSY_SMTP_TLS` | `starttls` | `starttls`, `tls` or `none`. |
| `NECROPSY_DEV_MAIL_LOG` | unset | `1` logs messages instead of sending them, recipient and links included. Local development only. |

Bad settings stop the server at startup instead of silently disabling mail.

With no SMTP host (and no dev log), registration and password reset are closed, since both depend on email. The admin can still sign in. The admin page shows the mail status and can send a test message.

## Email privacy

Email addresses are never displayed to anyone, including the admin and the user who owns them. `db::User` has no email field, so no page or JSON response can render one by accident. The settings page asks for a new address and never shows the current one.

An address is only used to send mail: verification links, password reset links, and notices when a password or email changes. The only code that reads a stored address is `Db::email_of`, used for those notices. Anyone with direct access to the database file can still read addresses, so protect the file.

Registration and password reset answer the same way whether or not an address is already registered, so those forms cannot be used to find out who has an account.

## Registration, login, reset

- **Register:** username, email, password. The account is created unverified and a link is emailed. It expires in 48 hours, and unverified accounts are deleted after 48 hours, which frees the username.
- **Login:** username and password. Unverified and disabled accounts cannot sign in. Wrong password and unknown user give the same answer, and an unknown user costs the same time as a wrong password.
- **Password reset:** a one-hour, one-time link emailed to the address. Resetting or changing a password signs the account out everywhere.
- **Change email:** needs the password. Available to the admin as well. A 24-hour link goes to the new address, and the old address gets a notice when it takes effect.
- **Delete account:** needs the password. Removes the user and their solves.

Rules: usernames are 3 to 24 characters of letters, digits, `-` and `_`, case-insensitively unique, and, for registration, names starting with `admin`, `root`, `necropsy`, `system`, `support`, `staff` or `moderator` are reserved. Passwords are 10 to 128 bytes. There are no composition rules.

## Sessions and tokens

- Passwords: Argon2id with the crate defaults, run off the async runtime.
- Session cookie `necropsy_session`: 256 random bits, `HttpOnly`, `SameSite=Lax`, `Secure` when `NECROPSY_BASE_URL` is https, 30 days.
- Sessions and email tokens are stored as SHA-256 hashes, so a database leak does not yield live sessions or reset links.
- Email tokens are 256 random bits, single use, and replace any earlier token of the same kind for that user.

## Abuse limits

Kept in memory and cleared on restart. Counted per 15 minutes:

| Action | Limit |
| --- | --- |
| Login attempts | 30 per client address, 10 per username |
| Registrations | 5 per client address |
| Password reset requests | 5 per client address |
| Password confirmations in settings | 10 per user |

The client address is the connecting address. Behind a reverse proxy that would always be the proxy, so set `NECROPSY_CLIENT_IP_HEADER` to the header your proxy fills with the real address, such as `X-Real-IP`. The proxy must always overwrite that header (Caddy: `header_up X-Real-IP {remote_host}`). Also set `NECROPSY_TRUSTED_PROXIES` to your proxy's address (IPs or CIDR ranges, comma-separated): the header is then believed only on connections from those addresses, so nobody who can reach the server directly can spoof it. Without it the header is believed from any connection, and the server warns at startup. Without a proxy, leave both unset: any client could otherwise choose its own address and dodge the limits. A value that is not an IP address is ignored, and for a list header such as `X-Forwarded-For` the last entry, the one your proxy appended, is used.

## CSRF

Every state-changing request must come from the site's own origin. `main.rs` rejects a POST whose `Origin` is not the site (or the request `Host`), or whose `Sec-Fetch-Site` is `cross-site`. Requests with neither header are not browser form posts and pass. The session cookie is also `SameSite=Lax`.

## Admin page

`/admin` is for the generated admin only: anonymous visitors are redirected to the login page and signed-in non-admins get a 404. It shows the mail status with a test send, and lists users by username, join date, status and solve count, with disable, enable and delete (after a confirmation page). Disabling ends the user's sessions. It never shows email addresses.

## Data

SQLite, created at `NECROPSY_DB` (default `necropsy.db`). Tables: `users`, `sessions`, `email_tokens`, `solves`. The schema version is in `PRAGMA user_version`. A background task removes expired sessions, tokens and unverified accounts hourly.

Requests are not logged by default. With `NECROPSY_LOG_REQUESTS=1` the log records the method, path and status of each request, never the query string, because email links carry one-time tokens there.


## Captcha

Creating an account and starting a live instance both ask for a [Cloudflare Turnstile](https://developers.cloudflare.com/turnstile/) check, so scripts cannot mass-register accounts or fill the range. Create a Turnstile widget in the Cloudflare dashboard for your domain (the site key is public, the secret is not) and set:

```bash
NECROPSY_TURNSTILE_SITE_KEY=0x4AAAAAAA...
NECROPSY_TURNSTILE_SECRET=0x4AAAAAAA...
```

Set both or neither: with neither, no captcha is asked and the server warns at startup. A check that cannot reach Cloudflare is treated as failed, so the site never lets someone through because the check was down. The token is verified on the server, once per use, together with the client's address.

Registration shows the widget in its form. The puzzle page loads the widget when someone presses "Spin up instance", and only a *new* instance asks: reopening a running one, or looking at its log, does not. The content security policy allows `challenges.cloudflare.com` for scripts and frames because Turnstile needs it. Login is not behind the captcha; it already has its own rate limits.
