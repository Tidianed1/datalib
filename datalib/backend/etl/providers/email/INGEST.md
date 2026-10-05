# JMAP Extract

`jmap-ingest` mirrors a JMAP mail account (RFC 8620 core + RFC 8621
mail) into a single doltlite raw store. Generic across JMAP servers —
tested against Fastmail (`api.fastmail.com`), works against any RFC
8620–conformant server in principle (Stalwart, etc.). The `email`
source's other two modes, the Gmail API and an mbox file, write the same
raw schema; [`docs/dev/email_download_modes.md`](/docs/dev/email_download_modes.md)
covers all three.

Each phase upserts upstream payloads as JSONB into per-type tables;
the full RFC 5322 `.eml` source of every email lands in the per-source
blob CAS (see "The raw store's shape" below). See `src/ingest/schema_raw.rs`
for the schema and
[`docs/dev/data_architecture_ingestion.md`](/docs/dev/data_architecture_ingestion.md)
for the rationale behind the table shape.

## Auth

`jmap-ingest` does not handle credentials directly — it shells out
to [`latchkey curl`](https://github.com/imbue-ai/latchkey), which
injects `Authorization: Bearer <token>` on every outbound request
based on the request's URL host. For Fastmail, the steps are in
[`docs/user/getting_your_data.md`](/docs/user/getting_your_data.md)
§"Fastmail".

For another JMAP server (Stalwart, etc.), register a latchkey service
for its host and store its token (the service name is only a label;
the URL host drives routing):

```sh
latchkey services register mail-example --base-api-url="https://mail.example.com/"
latchkey auth set mail-example -H "Authorization: Bearer $(pbpaste)"
```

Blob bytes come from the session's `downloadUrl`, which may be on a
different host than the API (Fastmail's is `www.fastmailusercontent.com`);
that host needs a credential too. Then run with
`--hostname mail.example.com`; session discovery does the rest.

## Run it

```sh
bazelisk run //datalib/backend/etl/providers/email:jmap_ingest -- \
    --out ~/backups/fastmail \
    --hostname api.fastmail.com
```

The store is `<out>/entities.doltlite_db`. Subsequent runs are
incremental — the state token from `Email/changes` is persisted
per-account in `sync_scope_state`, so only created / updated /
destroyed emails since the last run get touched. Force a full
re-enumeration with `--full-resync`.

To restrict to specific mailboxes, name them by their full label path
(`Work/Projects`), the way `only_extract_labels` does in a config:

```sh
jmap-ingest --hostname api.fastmail.com --out ~/backups/fastmail \
    --only-mailbox-labels "Inbox,Work/Projects"
```

The first run's `Mailbox/get` lands in the `mailboxes` table:

```sh
bazelisk build //third-party/doltlite:doltlite
bazel-bin/third-party/doltlite/doltlite -readonly ~/backups/fastmail/entities.doltlite_db \
    "SELECT id, name, role FROM mailboxes ORDER BY name"
```

Stock `sqlite3` cannot open the file;
[`docs/dev/doltlite.md`](/docs/dev/doltlite.md#getting-the-data-out-export-to-plain-sqlite)
has the one-pipe export.

## API surface used

| JMAP method        | Purpose                                                 |
|--------------------|---------------------------------------------------------|
| `.well-known/jmap` | Session discovery → `apiUrl`, `downloadUrl`, accounts   |
| `Mailbox/get`      | Full mailbox list (first run + fallback)                |
| `Mailbox/changes`  | Incremental: created / updated / destroyed mailbox ids  |
| `Email/get`        | Envelope of every touched email (no body: see below)    |
| `Email/changes`    | Incremental: created / updated / destroyed email ids    |
| `Email/query`      | Full enumeration when no state token exists             |
| `Thread/get`       | Thread membership for every touched threadId            |
| `downloadUrl`      | Each email's `.eml` bytes                               |

## Incrementality

State-token-first; falls back to enumeration on `cannotCalculateChanges`
or first run. Cursors persisted per `(account_id, type_name)` in the
shared `sync_scope_state` table under `jmap:<account_id>:state:<type>`
keys. `--full-resync` clears the cursor for this run only; the next
run re-establishes incremental sync from the post-resync state.
Widening `only_extract_labels` enumerates the newly admitted mailboxes
once, since `Email/changes` cannot surface mail that was already there;
a label path that matches no mailbox is a `problems` row.

Destroyed emails (per `Email/changes`) hard-delete the row, its
mailbox and keyword joins, its `email_blobs` edge, and their
bookkeeping. The bytes stay in the CAS — another email may share the
same `.eml` blob, and doltlite's history retains the prior state
either way.

A full enumeration is the one moment the mirror sees what was destroyed
while no cursor was replaying. A mailbox a full `Mailbox/get` does not
list comes off every email and its row goes, the same as one
`Mailbox/changes` reports destroyed. An `Email/query` walk that finished
and was not narrowed by `only_extract_labels` prunes the emails it did
not list, and the threads left with none.

## When part of a sync fails

Whatever part of a sync fails becomes a row in `problems`, and the
sync goes on with the rest. A row clears only when a later run tries
the same thing again and it works. The step fails only when nothing
useful is left to do: the store will not take a write, the credential
is refused, or the first listing fails with nothing stored to fall
back on.

**JMAP.**

- A `Mailbox/get` that fails is a `listing:Mailbox/get` row. The
  mailboxes an earlier run stored still file the mail.
- An `Email/query` walk that stops on an error is a
  `listing:Email/query` row. The emails it stored stay. Nothing is
  pruned, and no state token is saved, so the next run walks again.
- A `Thread/get` that fails is a `phase:Thread/get` row. The next run
  asks for every thread that has emails and no row of its own, even
  though none of those emails changed.
- An `.eml` that does not download is an `email_blobs:<edge id>` row,
  and every run tries again any `.eml` it does not hold. One over
  `blob_size_limit_bytes` is a warning (`over_size_limit`), not a
  failure.
- Some download failures end the blobs phase with an error: a refused
  credential (401/403), a retry loop that gave up, or twenty failures in
  a row. Any of these would fail every remaining `.eml` the same way.
  What downloaded before is kept.

**Gmail API.**

- A message that would not fetch is a `record:gmail_messages:<Gmail id>`
  row. The next run asks for it by id, even when the cursor has moved
  past it.
- A message that fetched but would not store gets the same row. A Gmail
  message's bytes never change, so fetching it again with the same build
  would only spend 20 quota units for the same answer. The build that
  failed is kept under `gmail:<account>:unstorable:<id>` in
  `sync_scope_state`, and only a different build (version or git hash)
  asks for the message again. A message deleted upstream drops its row.
- A message whose `.eml` was over the limit is a warning on its `.eml`.
  Once the limit allows it, a later run fetches the message again. If
  Gmail answers 404 for it then, the message was deleted, and it goes.
- A run that never reached an earlier failure, because it was stopped,
  hit the budget, or hit an error, keeps that failure's row.
- A `messages.list` walk that fails is a
  `listing:messages.list <label>` row. The other labels are still
  walked, nothing is pruned, and the cursor is held.
- A refused credential, a spent daily quota, or a retry loop that gave
  up ends the run with an error. What was fetched before it is kept.

**mbox.** A file that will not open, or whose read fails part-way, is a
`listing:mbox <file>` row. It is not stamped, so the next run reads it
again. Messages that will not parse are one `file:email/mbox:<file>`
row on their file, which stands until the file is read again. While
either kind of problem is present, the run deletes no message.
Rewritten files are then left unstamped, so the next run reads every
file again and prunes once all of them read cleanly. On a run that
reads every message, an `only_extract_labels` entry that no message
carries becomes a `config:` row.

## Rate limits

Fastmail doesn't 429 us in practice — JMAP's batch shape (one
methodCalls envelope = one HTTP request, regardless of how many
created/updated ids it carries) keeps the request count tame. A 429 or
502–504 is retried with backoff, honouring `Retry-After`, by the shared
HTTP layer (`datalib_etl::http::default_retryability`).

## Tests

**Nothing exercises the real JMAP wire format**: there is no recorded
JMAP fixture or live test (`playback_roundtrip.rs` is an empty
placeholder). `jmap_full_resync_prunes.rs`, `jmap_progress_countdown.rs`
and `jmap_run_problems.rs` replay hand-written answers in the shapes
RFC 8621 gives. `tests/email_tests/jmap_render.rs` builds a parsed store in
memory with real `.eml` bytes and renders it; `jmap_mbox.rs` runs the
mbox mode end to end over `tests/fixtures/mbox/star_trek.mbox`.

## The raw store's shape

One schema regardless of where the data came from — mbox and JMAP both
populate it, and the mbox path synthesizes a JMAP-shaped envelope so the two
are identical downstream.

### The `.eml` is the canonical body

The RFC 5322 `.eml` is the **complete backup** of a message: body, headers and
every MIME part, attachments included. It rides in the shared per-source CAS
keyed by `blob_id`, and everything else is metadata around it.

Concretely, there is no `email_attachments` table. The parts inside an `.eml`
are reachable by mail-parsing the bytes at render time, so we don't download
them into separate CAS entries during ingest. Both mbox and JMAP land *only
the `.eml`*.

### `emails` carries the envelope as `payload`

`EmailRow` is payload-shaped like every other entity table: the `id`/`payload`
pair plus promoted columns (time, subject, from/to/cc, message-id, threading
headers, the `.eml`'s blob ref). The payload is the JMAP `Email/get` envelope
— envelope only, since the body comes back from the `.eml`. The promoted
columns exist for indexing and cheap projection; the `mailboxIds` / `keywords`
join inputs are read back out of the payload.

### The `.eml` hash lives on `email_blobs`, not on `emails`

That column has a second writer — the blob-download pass backfills it after
the envelope row already exists — so it lives on its own CAS edge table, like
every other provider's attachment edge. That keeps `emails` single-writer, so
re-upserting a changed envelope (flag or move churn) never clobbers a stored
hash.

### Tables

| table | shape |
|---|---|
| `accounts`, `mailboxes`, `threads`, `emails` | payload-shaped entity tables, each with a paired `<table>_bookkeeping` sidecar; a mailbox's counts live in the sidecar's `volatile_payload` |
| `gmail_messages` | Gmail API mode only: Gmail's message id → the row it produced |
| `email_mailboxes`, `email_keywords` | N:M join tables with a synthesized `id` PK, refreshed delete-then-insert per email upsert; no sidecars |
| `email_blobs` | CAS edge carrying the `.eml` `blake3`, NULL until the bytes land |
| `ingested_files` | the shared per-file resume cursor (`file_checkpoint`, scope `email/mbox`) |
