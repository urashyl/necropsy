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

The admin has no email address, so password reset by email does not apply to it. The admin account cannot be disabled, deleted, or self-deleted from the UI.

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
- **Change email:** needs the password. A 24-hour link goes to the new address, and the old address gets a notice when it takes effect.
- **Delete account:** needs the password. Removes the user and their solves.

Rules: usernames are 3 to 24 characters of letters, digits, `-` and `_`, case-insensitively unique, and names starting with `admin`, `root`, `necropsy`, `system`, `support`, `staff` or `moderator` are reserved. Passwords are 10 to 128 bytes. There are no composition rules.

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

The client address is the peer address. Behind a reverse proxy, set `NECROPSY_TRUST_PROXY=1` to use the last `X-Forwarded-For` entry, which is the one your proxy appended. Do not set it without a proxy, or clients can choose their own address.

## CSRF

Every state-changing request must come from the site's own origin. `main.rs` rejects a POST whose `Origin` is not the site (or the request `Host`), or whose `Sec-Fetch-Site` is `cross-site`. Requests with neither header are not browser form posts and pass. The session cookie is also `SameSite=Lax`.

## Admin page

`/admin` is for the generated admin only: anonymous visitors are redirected to the login page and signed-in non-admins get a 404. It shows the mail status with a test send, and lists users by username, join date, status and solve count, with disable, enable and delete (after a confirmation page). Disabling ends the user's sessions. It never shows email addresses.

## Data

SQLite, created at `NECROPSY_DB` (default `necropsy.db`). Tables: `users`, `sessions`, `email_tokens`, `solves`. The schema version is in `PRAGMA user_version`. A background task removes expired sessions, tokens and unverified accounts hourly.

Server logs record the method and path of each request, never the query string, because email links carry one-time tokens there.
