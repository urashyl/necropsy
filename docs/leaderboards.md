# Leaderboards

Two boards, both public:

| Board | Route | From | Checked |
| --- | --- | --- | --- |
| Casual | `/leaderboard` | Every signed-in player's solves and Learn badges | No more than a solve already is |
| Ranked | `/leaderboard/ranked` | Each player's one ranked attempt at each live daily | One attempt per player per daily; the top explain their solves to a moderator |

Signed-out solves are kept only in the browser, so they are on neither board.

## Casual

One row per verified, enabled player with at least one solve, sorted by:

1. **Streak:** dailies solved on their own release day (in the solver's timezone at the time), on consecutive days ending today or yesterday. Solving an old daily does not count, as for the streak in the header.
2. **Solved:** every puzzle solved, any track.
3. **Badges:** Learn checkpoints passed ([learn.md](learn.md)).

Then by name. The top 100 are shown.

## Ranked

Live dailies only: a daily with a live instance ([live.md](live.md)). Each has its own board, chosen with `?pack=<id>` (default: the newest released live daily). A puzzle that is not a released live daily answers `404`.

### The ranked attempt

A player gets **one ranked attempt per live daily**:

- **It starts with their first instance of the daily on the daily's day**, in the player's own timezone. The attempt row is written in the same transaction as the instance, so no instance of that daily exists without it.
- **It ends once:**
  - **solved**, when they send the instance's flag while that instance is still running. The time is from the instance's start to the answer.
  - **failed**, when they stop the instance, or it runs out, without a solve. Restarting does not reopen it.
- **Nothing resets it.** No route deletes or edits an attempt. A database trigger refuses any change to a finished attempt's status, start, end, time or hints, and to the start of a running one, whatever the code asks.

Every other instance start of a live daily makes a **personal record** instead: a later attempt, or any attempt off the daily's day (in the archive). Personal records end the same way, are listed only on the player's own account page, and are on no board.

A live puzzle page tells a signed-in player, before they start, that starting the instance starts their ranked attempt, and afterwards whether it is running or over.

Only `POST /api/p/{id}/instance` (a player's own start) records attempts. An admin's test instances from `/admin` record none.

An instance whose container fails to start ([range.md](range.md)) is undone with the attempt it began, since the player never had it.

Attempts still marked running after their instance ended are settled as failed when anything reads them, and by the hourly housekeeping task.

### Board order

Solved ranked attempts that are on the board, fastest first, then the earlier finish. The page shows the place, player, time, hints used and the state of the entry's proof, and how many ranked attempts the daily has had in all.

### Removing an entry, and proofs

The top of each board explains its solves to a moderator, and moderators can take an entry off the board with a reason, or put it back. A removed, rejected or forfeited entry takes no place; its attempt, status and time are kept. The player's account page marks a removed entry "removed by a moderator". See [moderation.md](moderation.md).

## Storage

| Table | |
| --- | --- |
| `ranked_attempts` | `(user_id, pack_id)` unique: `started_at`, `ended_at`, `status` (`running`, `solved`, `failed`), `seconds`, `hints`, `hidden`, `hidden_by`, `hidden_reason`. Trigger `ranked_attempts_final` guards it. |
| `personal_records` | `id`, `user_id`, `pack_id`, `started_at`, `ended_at`, `status`, `seconds`, `hints`. |

Both go with the account when it is deleted.

## When something breaks

| Symptom | Cause, and what to do |
| --- | --- |
| A player says their ranked attempt failed though they solved it | The flag was sent after the instance ended (stopped or ran out), so it solved the puzzle but not the attempt. The answer reply has no `attempt` field in that case. Working as intended. |
| A player's start failed and they ask for their attempt back | A failed container start already undoes it. Otherwise attempts cannot be reset: that is the point of them. |
| An entry is clearly not fair play | A moderator removes it with a reason ([moderation.md](moderation.md)). Do not edit the database: the trigger refuses it, and dropping the trigger to get round it is a change to the scoring rules, which needs a code change and its review. |
| The ranked board is empty on a live daily's day | Nobody has solved it in a ranked attempt yet, or the site has no live domain set (`NECROPSY_LIVE_DOMAIN`), so live dailies are hidden. |
| The server will not start: "database schema version 10 is not supported" | An older build is running against a newer database. Deploy the current build. |
