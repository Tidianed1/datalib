# The `google_takeout` source

An unpacked [Google Takeout](https://takeout.google.com) export, read
off `export.path` (the directory holding `Takeout/`); the feed switches
sit beside it in the same table. There is no API and no network: the
download step walks a directory tree the user exported and unzipped
themselves.

## Every feed is opt-in, and off by default

`SyncFlags` (`src/ingest/mod.rs`) is one boolean per feed, mirrored by
the `export` table in `google_takeout_config`, and `Default` sets every
one of them to `false`. A user has to enable each
feed consciously.

That is deliberate, and it is the one rule to preserve if you touch
this provider. A Takeout export is whatever the user asked Google for,
so its subtrees are wildly uneven — an export may hold nine years of
YouTube history and no Maps data at all — and silently ingesting
everything present would make "add this source" an unbounded promise.
`google_voice_include_spam` is a second-level switch under
`google_voice` for the same reason: spam is bulky and only useful for
parser hardening.

`tests/fixture_walk.rs`'s `sync_flags_default_disables_everything` is
the regression test: it runs the walk with a default `SyncFlags` and
asserts that the Maps, YouTube, Chat and Gemini feeds land nothing.

## The feeds and what they write

| flag | raw tables |
|---|---|
| `maps_reviews` | `maps_reviews` |
| `maps_saved_places` | `maps_saved_places` |
| `maps_photos` | `maps_photos` (bytes in the blob CAS) |
| `youtube_watch_history` | `youtube_watch_history` |
| `youtube_subscriptions` | `youtube_subscriptions` |
| `google_chat` | `chat_groups`, `chat_users`, `chat_messages`, `chat_attachments` |
| `gemini_apps` | `gemini_activity`, `gemini_attachments` |
| `google_voice` | `voice_messages`, `voice_bills`, `voice_greetings`, `voice_attachments` |

`DATA_TABLES` and `EDGE_TABLES` in `src/ingest/schema_raw.rs` list the
tables the store is created with. Google Voice keeps its own
`schema_raw` under `src/ingest/google_voice/`.

Ids are uuidv5 under a per-provider namespace, minted from a recipe
string (`"maps_review:{ftid}:{date}"`, `"youtube:watch:{id}:{ts}"`);
see `ns_id` in `schema_raw.rs` and
[`docs/dev/entity_ids.md`](../../../../../docs/dev/entity_ids.md).

## When part of a sync fails

A feed reads only the files that changed since it last stamped them
(`file_checkpoint`), so what failed has to be either read again next run
or stamped with its problem. Each kind of failure is a `problems` row:

- **A feed that fails as a whole** is a `phase:<feed>` row. It stamped
  nothing, so the next run tries it again and the row goes with the run
  that succeeds. The other feeds run regardless.
- **A file that will not read or parse** — a Chat `user_info.json`,
  `group_info.json` or `messages.json`, a Maps photo sidecar, a Voice
  record, `Bills.html` or a greeting — is a `listing:<feed> <path>` row
  and is left unstamped, so the next run reads it again. The rest of the
  feed lands.
- **Records a single-file feed cannot key** (a review or saved place
  with no place id or date, a subscription row short of columns, a
  watch with no video id), or a Maps file with no `features` list, are
  one `file:google_takeout/<feed>:<path>` row. The file is stamped, so
  the row stands until the file changes and is read again.
- **A Chat or Gemini attachment** that is not in the export is a
  `not_found` warning on its edge (`chat_attachments:<message>#<name>`,
  `gemini_attachments:<activity>#<name>`), and one that is there but
  will not read an error. The file naming it is stamped, so each run
  looks again for every edge with no bytes, and the row goes when the
  bytes land.
- **A Maps photo whose media is missing or unreadable** lands its row
  without bytes and a `maps_photos:<stem>` problem, and its sidecar is
  left unstamped so the next run looks for the media again.
- **A Voice attachment** that is missing or unreadable is a
  `listing:voice <path>` row on the file naming it, which is left
  unstamped: a Voice message names only the attachments it read, so
  reading the file again is the only way to add one.
- **Deletions are held back** when a walk had errors (`listing:files`)
  or, for Voice, when files were removed or rewritten but one could not
  be read (`listing:removed_records`). A rewritten Voice file keeps its
  old stamp on such a run, so the next one still reads every file and
  deletes what the rewrite dropped.

Attachments are flushed before the files naming them are stamped, so a
flush that fails leaves the files to be read again. A run that was
stopped reports nothing, and clears nothing.

## Tests

`tests/fixture_walk.rs` points the downloader at the checked-in
TNG-themed Takeout tree (`tests/fixtures/Takeout/`) and asserts, per
Maps, YouTube, Chat and Gemini feed, the rows that land — counts, a
sample of the parsed content, the CAS digest for the photo feed — and
that a second walk over an unchanged tree ingests nothing. Google
Voice's parser and walk are tested in `src/ingest/google_voice/`.
