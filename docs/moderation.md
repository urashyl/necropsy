# Moderation

The top of each ranked board ([leaderboards.md](leaderboards.md)) has to explain its solves. Moderators read the explanations and remove entries that are not fair play. Admins choose the moderators. Every one of these actions is in the audit log.

## Proofs

The top `NECROPSY_RANKED_TOP_N` entries of each live daily's ranked board are asked for a **proof**: the player's own explanation of how they found the flag. The default is 10.

1. **Asked.** When an entry enters the top N, the site makes a proof request for it and emails the player a link to `/proof/<pack>`. A player who has just solved gets the link in the answer reply and on the puzzle page instead. The request is shown on the player's puzzle pages, account page and the ranked board until they start it.
2. **Started.** The page explains the rules. Pressing **Start** starts a 90-second clock kept by the server. The page says "AI-generated explanations are not accepted." The box locks when the time runs out.
3. **Sent.** A proof of 20 to 2000 characters, sent before the deadline (with 2 seconds' grace for the request to arrive), goes to review. A late one gets `409`.
4. **Reviewed.** A moderator approves it, and the entry shows **Verified**, or rejects it with a reason, which the player sees on the proof page.

A proof request has to be started within 24 hours. **A proof not started in time, or not sent in time, is forfeited.** A rejected or forfeited entry leaves the board. Its attempt and time are kept, and moderators still see the entry, struck through, with the reason. Whoever moves up into the top N is asked in turn.

Once asked, a proof stays due even if faster solves later push the entry below the top N.

The board's **Proof** column shows each entry's state: **Proof due**, **Under review** or **Verified**. Below the top N, entries that were never asked show nothing.

Late proofs are forfeited when a board, a proof or the review queue is read, and by the hourly housekeeping task, which also asks anyone newly in the top N.

| Setting | Default | |
| --- | --- | --- |
| `NECROPSY_RANKED_TOP_N` | `10` | How many places at the top of each board need a proof, 1 to 1000. The server will not start with another value. |

## Moderators and admins

| | Player | Moderator | Admin |
| --- | --- | --- | --- |
| Review proofs (`/mod/proofs`) | | yes | yes |
| Remove or reinstate a board entry | | yes | yes |
| See removed, rejected and forfeited entries on the board | | yes | yes |
| Name moderators (`/admin/moderators`) | | | yes |
| Read the audit log (`/admin/audit`) | | | yes |
| Every other admin page and action | | | yes |

A moderator who opens anything else under `/admin` gets `404`, as a player does. Signed-out visitors are sent to log in. Moderators have a **Moderate** link in the header, and admins reach the same pages from `/admin`. The admin account cannot be made a moderator; it can moderate already.

Removing an entry needs a reason of up to 200 characters, and so does rejecting a proof. Approving needs none.

## Audit log

`audit_log` gets one row for each:

- proof approved or rejected;
- proof forfeited (actor "the site");
- board entry removed or reinstated;
- moderator added or removed.

Each row holds the time, who did it, what, the player, the puzzle and the reason. It is written in the same database transaction as the change itself, so a change without its row, or a row without its change, cannot happen. Names are copied into the row, so it outlives the accounts it mentions. The site has no way to edit or delete a row.

`/admin/audit` shows the newest 200.

## Routes

| Route | Who | |
| --- | --- | --- |
| `GET /proof/{pack}` | The player asked | The proof: its rules and Start button, the form while the clock runs, or its result. `404` if they were not asked. |
| `POST /proof/{pack}/start` | The player asked | Starts the 90 seconds. Starting again changes nothing. |
| `POST /proof/{pack}` | The player asked | Form field `text`. `400` if it is not 20 to 2000 characters, `409` if not started or too late. |
| `GET /mod/proofs` | Moderators | Proofs waiting for review, oldest first. |
| `POST /mod/proofs/{id}/approve`, `.../reject` | Moderators | `reject` needs `reason`. `404` if the proof is not waiting for review. |
| `POST /mod/board/{pack}/{user}/remove`, `.../reinstate` | Moderators | `remove` needs `reason`. `404` if there is no such ranked attempt. |
| `GET`, `POST /admin/moderators` | Admins | The list. The form takes `username` and `action` (`add` or `remove`). `404` for an unknown name. |
| `GET /admin/audit` | Admins | The newest 200 audit entries. |

## Storage

| Table or column | |
| --- | --- |
| `users.moderator` | `1` for a moderator. |
| `proofs` | One per `(user_id, pack_id)`: `offered_at`, `opened_at`, `deadline`, `text`, `submitted_at`, `status` (`open`, `pending`, `approved`, `rejected`, `forfeited`), `reviewer_id`, `reviewed_at`, `reason`. |
| `audit_log` | `at`, `actor_id`, `actor`, `action`, `target_id`, `target`, `pack_id`, `reason`. No foreign keys. |

## When something breaks

| Symptom | Cause, and what to do |
| --- | --- |
| A player says they never got the email | Mail may be off or failing (see [accounts.md](accounts.md)). The request is also shown on their pages when they sign in. If the 24 hours have passed, the proof is forfeited, and a moderator can reinstate the entry only if it was *removed*: a forfeited proof cannot be reopened. |
| A player ran out of time because the page was slow | The deadline already allows 2 seconds for the request. Beyond that, the proof stands as forfeited. |
| A moderator approved or rejected by mistake | Reviews are final. If a rejection was wrong, remove nothing; tell the player, and note it. A wrongly approved entry can be removed with a reason. Both show in the audit log. |
| A moderator should lose access | Remove them on `/admin/moderators`. Their sessions stay signed in, but the next request has no moderation access. |
| The server will not start: "NECROPSY_RANKED_TOP_N must be a number from 1 to 1000" | Fix the setting, or remove it for the default of 10. |
