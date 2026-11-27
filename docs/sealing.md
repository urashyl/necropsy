# Sealed packs

The repository is public, so an unreleased daily cannot sit in it as plaintext. Unreleased dailies are **sealed**: encrypted in `packs-sealed/`, opened only by the production site, which holds the master key. Each pack's own key is published once its date is over in New York; from then on anyone can open that pack from the repository, and no other.

```text
packs-sealed/
  manifest.toml        id, number, difficulty, date (or fallback), key version, sha256
  wrap-keys/v1.pub     the public key packs are sealed to
  <id>.sealed          one sealed pack each
  README.md
packs-src/             where authors write packs: gitignored, never committed
```

## Keys

- **The master key.** One secret, held only by production: a file the site makes on its first start if there is none (below). Never in the repository, never in CI, never given to authors. It is separate from the server secret ([generators.md](generators.md#the-server-secret)): a leaked master key spoils upcoming puzzles, but still reveals no player's answer.
- **The wrap key pair.** For key version N, the secret half is `HKDF-SHA256(master, info "necropsy:pack-wrap:vN")`, used as an X25519 private key. The public half is committed as `packs-sealed/wrap-keys/vN.pub` and served at `/api/packs/wrap-key`.
- **A key per pack.** Sealing makes a random 256-bit key K. The pack (every file of its directory) is encrypted with K using AES-256-GCM. K is sealed to the wrap public key in an X25519 sealed box, the same construction as libsodium's `crypto_box_seal`. Both go in the `.sealed` file, so no per-pack secret is stored anywhere else.
- **The header** holds the pack id, the key version, the public key's fingerprint (the first 8 bytes of its SHA-256) and the nonce, in the clear. It is the AEAD's associated data, so a K opens only the pack it was made for: putting one pack's header on another's ciphertext, or editing it, makes decryption fail.

## Writing and submitting a pack

1. Write the pack in `packs-src/<id>/`, like any pack (see [pack-format.md](pack-format.md)), with `track = "daily"` and its `date`. A generated pack builds with `pack-tool gen-build packs-src <id>` and is checked with `check-gen packs-src`.
2. **Test it on your own site.** A local site makes its own master key on first start, so it has its own wrap key. With `NECROPSY_BASE_URL=http://localhost:8080` in your `.env`, in a scratch copy of the repository, run `pack-tool fetch-wrap-key` to save your dev key, then seal and play the pack there. Production keys are never needed for this.
3. **Seal it to production's key:** `pack-tool seal <id>`. It loads and checks the pack, fetches the site's wrap key, checks it is the committed current one, and writes `packs-sealed/<id>.sealed` and its manifest entry.
4. **Open a pull request** with only those two files. `packs-src/` is gitignored, and CI fails if anything from it, a `.env`, a `*.key` file or a server secret is committed.

`pack-tool seal` asks for the wrap key at, in order:

1. `--site <url>`;
2. `NECROPSY_BASE_URL` from the environment, or from a `.env` file in the current directory;
3. `https://necropsy.dev`.

It refuses a site whose key is not the committed one for that version, because the production site could not open a pack sealed to it. Loopback sites (`localhost`, `127.0.0.1`) are reached without a proxy.

`--fallback` seals a daily without a date: a spare pack released on the date of a dated pack that fails its checks (below).

### Endpoints

| Route | |
| --- | --- |
| `GET /api/packs/wrap-key` | `{"version", "public_key", "fingerprint", "scheme"}`: the key to seal to now. |
| `GET /api/packs/keys` | `{"released": [{"id", "date", "key", "key_version", "released_at"}], "upcoming": [{"id", "date", "released_at"}]}`. |
| `GET /api/packs/{id}/key` | A released pack's key, or `404` with `released_at` until then. |

`pack-tool open <id>` opens a sealed pack into `packs-src/<id>`: with the master key if one is set (an admin reviewing a submission), otherwise with its released key from the site.

## CI

Pull requests run without secrets (see [ci-cd.md](ci-cd.md)). `pack-tool check-sealed packs packs-sealed` checks, with no key:

- every file in `packs-sealed/` is a `.sealed` file, `manifest.toml`, `README.md` or a wrap key: no plaintext pack is committed there;
- every `.sealed` file parses (header, key version, fingerprint, sealed K, ciphertext) and matches its manifest entry's id and sha256, and every entry has its file;
- ids are unique across sealed and plaintext packs, difficulty is 1 to 5, dates and daily numbers collide with no other daily's, and each entry has a date or is a fallback;
- each pack is sealed to the **current** committed wrap key: the newest `wrap-keys/vN.pub`, with a matching fingerprint.

`main` is protected by a ruleset ([ci-cd.md](ci-cd.md#branches)): pull request required, CI required. Before accepting submissions from other authors, an admin sets the ruleset's required approvals to 1, so only an admin's review merges a pack. An admin may open a submission (`pack-tool open`) to read its generator before approving.

## On the site

| Setting | Default | |
| --- | --- | --- |
| `NECROPSY_SEALED` | `packs-sealed` | The sealed packs. |
| `NECROPSY_MASTER_KEY_FILE` | `master-key` next to the server secret | A file holding the master key: `1:<64 hex>`, one per line or comma-separated during a rotation. Made, with a new random v1 key and mode 0600, if it does not exist. |
| `NECROPSY_MASTER_KEY` | unset | The key itself, in the environment. Used only when `NECROPSY_MASTER_KEY_FILE` is unset; then no file is read or made. |

The Docker image keeps the key in the data volume (`/data/master-key`), and the installed service in its state directory, beside the database and the server secret. **Back it up with them**: a new key cannot open any pack sealed to the old one. A key file that exists but is malformed stops the server; it is never replaced.

At startup the site logs a warning when it makes a key, and when its wrap key is not the one committed in `packs-sealed/wrap-keys/` (expected on a development site). It then opens every pack in the manifest and checks what CI could not: it decrypts, loads, agrees with its manifest entry, and its generator passes the same checks as `check-gen`, run in the generator sandbox. Passing packs are dailies like any other, shown from their date in each player's timezone.

**A pack that fails** is skipped, logged as an error, and listed on the admin page with the reason. The next fallback from the manifest takes its date and daily number. Without a fallback left, there is no daily that day, and the admin page says so. Checks run at every start, so a submission is checked on the deploy that merges it, ahead of its date.

**Keys are released** when the daily's date is over in New York: midnight at the end of its date, America/New_York, 05:00 UTC in winter and 04:00 UTC in summer (daylight saving: the second Sunday of March to the first Sunday of November). Players further west, such as in Hawaii, may still be on that date when it is released.

A released pack can be opened by anyone from then on. That is intended: it is a past puzzle, playable from the archive, and its explainer and generator are its write-up. Generated packs keep per-player answers after release, so knowing the generator does not give anyone else's answer.

## Setting up production

1. Deploy. The site makes its master key on first start (log: "created a new master key"). Back the file up.
2. `pack-tool fetch-wrap-key --site https://necropsy.dev`, then commit `packs-sealed/wrap-keys/v1.pub` through a pull request. Until it is merged, `pack-tool seal` refuses to seal to production.

To bring a key made elsewhere instead (`pack-tool new-master-key <file>`), put the file at the key path, or set `NECROPSY_MASTER_KEY_FILE`, before the first start.

## Rotating the master key

1. Make a new key: `pack-tool new-master-key new.key`, and give it the next version: edit it to `2:<hex>`.
2. Put both in production's key file, one per line (`1:...` and `2:...`). Deploy. The site now serves wrap key v2 and still opens v1 packs.
3. On a machine with that key file: `NECROPSY_MASTER_KEY_FILE=both.key pack-tool rewrap` reseals every unreleased pack to v2 with its same K, and writes `wrap-keys/v2.pub`. Commit and merge it.
4. Once nothing is sealed to v1, remove v1 from the key file.

Rotation protects packs from then on. **It cannot undo a leak**: anyone who copied the old master key and the old `.sealed` files from the repository's history can still open those packs. After a leak, assume every pack sealed to the old key is public, and replace the ones not yet released.

## When something breaks

| Symptom | Cause, and what to do |
| --- | --- |
| The admin page lists a sealed pack as skipped | The reason is next to it. "does not match its manifest sha256": the file and manifest disagree; seal again. "sealed to another v1 key": it was sealed against another site; seal again against production. "its generator fails its checks": the message is `check-gen`'s; fix and seal again. |
| "no fallback left" | A dated pack failed and the manifest has no spare. Seal a fallback (`pack-tool seal <id> --fallback`) and deploy, or fix the failed pack. |
| Every sealed pack is skipped as "sealed to another v1 key" after a move or restore | The site made a new master key because the old file was not at the key path. Stop it, put the backed-up file back (replacing the new one) and restart. Without a backup, those packs cannot be opened: seal them again from their sources, after committing the new wrap key. |
| The log says the wrap key is not committed | Normal before setup step 2; otherwise the site is using a key other than production's (see the row above). |
| `pack-tool seal` says the key is not committed | Production's wrap key is new. An admin runs `pack-tool fetch-wrap-key --site https://necropsy.dev` and commits `packs-sealed/wrap-keys/vN.pub`. |
| CI: "sealed to v1, but the current wrap key is v2" | A rotation is under way. Seal against the current key, or wait for the admin's rewrap to merge. |
| The server will not start: "master key v1 is not 64 hex digits" | The key file or variable is malformed. Restore it from the backup. |
