# Sync state: what is owed is what upstream listed, minus what we hold

**Status: decided 2026-10-05; step 0 of §7 is built.** This is the design and
the order of work. It came out of the audit in
[`audits/2026-10-05_loose_ends.md`](../audits/2026-10-05_loose_ends.md)
and a read of the four downloads with the most resume state (Slack,
email, Notion, Garmin). The bugs in §6 were traced in the code by one
reader each and are **not reproduced**; each wants a failing test before
its fix. Line numbers are at `c68906bff`. Where this file and the tree
disagree, the tree wins.

## 1. The problem

Every download keeps its own idea of what is left to do. The tree has
resume cursors, sync tokens, sweep markers with a time limit, a
`last_error` column read back as a retry list, `problems` rows read back
as a retry list, tables of owed and failed items, a record of the config
a cursor was walked under, and sets held only in memory. Most providers
use several at once.

Nearly every bug this has produced is one mistake:

> **A stored row doubles as "this item is done."**

A conversation is stamped with its new update time before its
attachments are stored, and the next run's skip-check reads that stamp.
A channel's newest stored message is taken to mean the channel is
fetched up to there. A thread's root is stored before its replies. A
file is stamped as read before the rows built from it exist. In each
case a run that stops between the two writes leaves a store that reads
as complete, and no later run finishes it.

## 2. The rule

Keep two facts apart, and compute the rest.

- **Listed:** what upstream says exists, and at what version.
- **Held:** what we have fetched, and the version it satisfies.
- **Owed** is the difference. It is a query, never a stored fact.

Nothing is marked done. Having the content at the listed version *is*
done, so "done" cannot be written early, and a step cannot be forgotten
because there is no step. The store is correct at every transaction
boundary: whatever is not held at its listed version is, by
construction, still owed.

The healthiest code in the tree already works this way. JMAP decides
which message bodies to download by asking the store which emails have
no stored body (`email/src/ingest/mod.rs:1196-1213`). Every hole in §6
is a place where that question cannot be asked, because what upstream
listed was never stored apart from what we fetched.

### What is stored

1. **The listing.** One row per item: its key and its version. The
   version is upstream's when it gives one (an update time, a thread's
   newest-reply stamp, an etag). When upstream only says "this changed",
   as a delta does, the version is a counter the listing bumps each time
   the item is named.
2. **On what we hold, the version it satisfies.** One stamp per call
   that can fail on its own. A Notion page's body and its comments come
   from two calls, so the page carries two stamps. The stamp is written
   in the transaction that stores the content it describes.
3. **For a range, that we looked.** "Which stretch of this channel have
   I not read?" cannot be derived from the messages. Where the range
   divides into fixed units (a day, a window), store a row per unit even
   when it came back empty. Where it does not (pages of history), store
   how far the walk has covered, in the transaction that stores each
   page.
4. **Attempts.** A count and the last error on the item, in the
   `_bookkeeping` sidecar every raw table already has.

### What is derived, and never stored

- **What is owed**, per kind of item.
- **Queue depth**: the size of that query. It is exact at any moment,
  and for a calendar-driven source it is known before the first request.
- **What is failing**: owed items with an attempt count above zero.
- **What a config change newly requires.** Widening `since` or turning
  on file download changes what is listed or wanted, so the difference
  grows by itself. There is no cursor for the change to hide behind.
- **What is blocked.** A file over the size limit, or a message this
  build could not store, is owed but not asked for. That is a condition
  on stored facts (its size, the build that failed) and the current
  config, so raising the limit or shipping a new build re-arms it with
  no bookkeeping.

### What stays policy in the fetch loop

Order (new mail before a year's backlog), batching, back-off, stopping a
kind after N failures in a row, and treating a refused credential as the
end of the run. None of it is stored.

### The two ways a listing is produced

- **A delta.** Upstream names what changed since a token, deletions
  included. One transaction writes the listed rows, applies the
  deletions and saves the new token. The token can advance at once,
  because what the delta named is durable whether or not it has been
  fetched.
- **An enumeration.** Upstream lists everything, or everything in a
  window. Something absent from it is gone **only if the enumeration
  reached its end**; one that did not deletes nothing. Almost no
  upstream has paging that survives between runs, so an unfinished
  enumeration starts again next run. That costs only the listing calls:
  what it already listed is stored, and what was fetched is held.

  Where pages are strictly ordered (Slack's history by timestamp), each
  page covers a known span, so "stored in this span, absent from this
  page" is decided and deleted in that page's transaction, with no set
  carried to the end of the walk.

## 3. What this deletes

- Resume cursors for enumerations, and the per-phase date cursors.
- Retry lists: the `last_error` queries, the owed and failed tables,
  `problems` read back as a queue.
- Skip-checks that compare a stored row against the listing.
- In-memory "seen", "changed", "attempted" and "touched" sets that a
  stop or an error loses.
- `scope_config`, the record of the config a cursor was walked under,
  and `lint_repo.py` check 8 that polices it. They exist because a
  cursor can swallow a config change; with no cursor there is nothing to
  swallow. AGENTS.md §"A cursor is only valid under the config that set
  it" goes with them.

Delta tokens stay. So do sweep markers that say "do not re-list more
than once every N hours": they schedule a listing, they do not record
progress.

## 4. What stays outside

- **Local sources.** Reading a folder or an export is fast enough that
  each run can read everything and commit once. All-or-nothing in one
  seal has no half-done state, so these sources need no listing, no
  stamps and no `file_checkpoint`. Deleting by absence is valid because
  the pass read everything, and is skipped when the walk had errors. The
  host-wide fingerprint cache stays: it is a cache, and losing it costs
  time, never correctness. Before switching a source over, time it on a
  large input (a multi-gigabyte mbox, a big media library, a Takeout
  archive).
- **Whole-snapshot sources** (the SQLite mirrors, a Signal backup):
  replaced in one seal, as now.
- **`problems`** keeps two jobs: a warning about something we do hold (a
  body cut short on purpose, comments the credential may not read), and
  what a run could not do as a whole (a listing, a phase). An error on
  something still owed needs no row of its own; it is the owed item and
  its attempt count. How the Manage row counts those is a detail to
  settle in step 1.

## 5. What this does not fix

Each is a property of the upstream. We take a well-formed answer at its
word and do not add requests to check it: a wrong deletion in a raw
store is recoverable from doltlite history. A malformed answer is a
different thing, and is not a listing at all (C1 in §6).

- **A listing that is not complete.** Notion's search by edit time never
  reports a deletion and can miss a page shared late. Only a periodic
  full pass would catch those.
- **Paging by offset.** Garmin's activity list can skip an item when
  something is deleted mid-walk, which makes a live activity look gone
  until the next run lists it again. Accepted.
- **Nothing reports the change.** Slack has no listing that says an old
  thread got a new reply. A refresh window is the only producer, which
  is why it defaults to 30 days.

## 6. Bugs this closes

Found while reading the four providers. **Traced** means one reader
followed every line of the path. **Suspected** means a step rests on
something not checked, named in the row. None is reproduced. Each is
fixed in the step of §7 that moves its provider, with a test written
first and watched failing.

### Slack (`etl/providers/slack/src/ingest/`)

| # | Bug | Evidence | Closed by |
|---|---|---|---|
| S1 | A history walk interrupted after its first page loses everything older, for good. History is newest-first; the failed walk is caught and the run succeeds, so page 1 is sealed; the next run resumes from the newest stored message. | traced; page order from Slack's docs. `mod.rs:1176`, `:1207`, `:1746-1749`; `db.rs:551-571` | coverage per channel, written with each page |
| S2 | The refresh-window prune deleted thread replies: it compared every stored message in the window with what history returned, and history does not return replies. | **fixed in step 0**, with a test that failed first (2 replies deleted with nothing changed upstream) | — |
| S3 | A thread root stored without its replies is not listed again. | traced. `mod.rs:1064-1085` | root's listed newest-reply stamp vs the stamp its replies were fetched for |
| S4 | A thread's retry mark is erased when a refresh re-writes its root, before the retry. | traced. `bulk.rs:83` | no retry mark |
| S5 | Attachment rows are lost on a stop mid-page, on a failed flush (a `warn!` only), and on a kill before the flush. | traced. `api.rs:353`, `mod.rs:872-874` | the edge row is written with the message; its bytes are owed |
| S6 | One permanently failing channel means the `since`/`media` record is never written, so a media-on full walk repeats every run. | traced. `mod.rs:1782-1789` | no record |
| S7 | Channels and users are never pruned; a channel left upstream fails every run. | traced | a complete `conversations.list` deletes by absence |
| S8 | With `refresh_window_days = 0`, then the default for a configured source, a new reply on an old thread was never seen. | **fixed in step 0**: the default is 30 | — |

### Email (`etl/providers/email/src/ingest/`)

| # | Bug | Evidence | Closed by |
|---|---|---|---|
| E1 | JMAP: when the body-download phase gave up (20 failures in a row, a refused credential), every body downloaded in that run was discarded: the phase flushed, then returned an error. | **fixed in step 0**, with tests that failed first: bodies are written and sealed as they land, and a give-up is a `phase:eml_download` row and `Ok` | — |
| E2 | JMAP: any error in `Email/changes` triggers a re-download of the whole account, not only "cannot calculate changes". | traced. `mod.rs:787-792` | the delta only lists; a failed fetch leaves an item owed |
| E3 | JMAP: a widened label filter whose backfill fails is recorded as satisfied and never retried. | traced. `mod.rs:563-567`, `:399-404` | no record; the wider filter lists more |
| E4 | JMAP: the Email state token is saved before the prune it ends. Unreachable today only because an error discards the unsealed tail. | traced. `mod.rs:1065-1067` vs `:783`/`:807` | token and deletions in one transaction |
| E5 | JMAP: an interrupted first enumeration never resumes; a run always stopped early never gets a token, a thread row or a body. | traced. `mod.rs:1031`, `:1129` | listed rows persist; fetch work carries over |
| E6 | Gmail: label changes on mail already mirrored never land. The fetch skips anything already held. | traced. `gmail_api/mod.rs:531-548`, `:903` | a relabel bumps the listed version |
| E7 | Gmail: one message that always fails holds the cursor forever. | traced. `gmail_api/mod.rs:956`, `:212-214` | the cursor advances with the listing |
| E8 | Gmail: a kill after a sealed flush and before the thread rebuild leaves emails with no thread row and no repair. | traced. `gmail_api/mod.rs:431`, `:1095` | thread membership written with the email |
| E9 | Gmail: `destroy` deletes the id mapping and the email in separate steps; an error between strands the email. | suspected (second-hand). `gmail_api/mod.rs:1140-1161` | one transaction |

### Notion (`etl/providers/notion/src/ingest/`)

| # | Bug | Evidence | Closed by |
|---|---|---|---|
| N1 | A failed comments listing is recorded on the page row, and the page's next write clears it before comments are asked again. | traced. `mod.rs:907`, `db.rs:166` | a comments stamp of its own |
| N2 | `max_pages` moves the search mark past pages it never fetched, with no problem row. | traced. `mod.rs:510-512`, `:1207-1210` | the mark only bounds the listing; unfetched pages are owed |
| N3 | One seal per run: a kill loses the whole run. | traced. `processor.rs:63` | seal as pages land |
| N4 | The search mark is exclusive at equal seconds, so a page edited in the same minute as the mark can be missed. | compare traced; impact suspected (Notion's rounding, from memory). `mod.rs:487` | inclusive compare |
| N5 | Deletions are never learned in search mode. | traced. `mod.rs:460` | not closed (§5) |
| N6 | In roots mode `<database>` children are parsed and never walked; the `databases` config has no reader. `INGEST.md:71-72` says otherwise. | traced | walk them, or correct the doc |
| N7 | An anchor fetch failure reaches only `debug!`. | traced. `mod.rs:664-669` | an owed item |

### Garmin and YoLink (`etl/providers/{garmin,yolink}/src/ingest/`)

| # | Bug | Evidence | Closed by |
|---|---|---|---|
| G1 | A changed activity whose detail refetch fails or is cut off keeps its old detail for good: the new listing entry is stored first, and "changed" lives only in memory. | traced. `mod.rs:961-982`, `:1049-1053`; `db.rs:322-331` | the detail records the listing version it was fetched for |
| G2 | The same for a FIT file marked unreadable until the activity changes. | traced. `mod.rs:1002` | same |
| G3 | Turning `activity_files` on later fetches files only for activities in the refresh window. | traced. `mod.rs:562-582` | owed is every listed activity with no file |
| G4 | A widened `since` plus one permanently failing day re-walks all history every run. | traced. `mod.rs:266-270`, `:343`, `:590-592` | no cursor, no record |
| G5 | A failed token exchange mid-run is not treated as an auth failure; each metric burns ten days as failed. | traced. `auth.rs:251-258`, `mod.rs:1336-1339` | type the error; fix with step 1 |
| G6 | Today's wellness bundle is fetched once and never again, though the day is not over. | suspected; needs a live check. `mod.rs:1149-1156` | "held" means fetched after the day ended |
| G7 | Offset paging while the list shrinks can prune a live activity. | suspected; depends on Garmin's order | accepted (§5) |
| Y1 | A device that stops reporting is re-requested from its last reading to now on every run, growing without bound: an empty window leaves no trace. | traced. `yolink mod.rs:746-757`, `:659-687` | a row per window, empty or not |

### ChatGPT and Claude

| # | Bug | Evidence | Closed by |
|---|---|---|---|
| C1 | ChatGPT: a listing page with no `items` key read as a complete, empty listing and pruned every conversation. | **fixed in step 0**, with a test that failed first (all three conversations pruned) | — |
| C2 | ChatGPT and Claude: the conversation row and its freshness stamp are committed before its attachment rows. Harmless while an error discards the unsealed tail, which is why sealing at an error was taken back out of #1008. | traced. `chatgpt mod.rs:566-578`; `claude mod.rs:1467-1469` | attachments are listed in the conversation's transaction |

## 7. Order of work

Each step is a PR. A bug's test is written first and watched failing.

**Step 0 — fixes that should not wait. Done.** C1, E1, S2, and with S2
settled, S8.

**Step 1 — Garmin.** It needs no new stored state: days are listed by
the calendar and held as rows, an empty day included; activity detail
and files record the listing version they were fetched for. The four
date cursors, the failed-day queries, the `changed` set and the config
record go. Closes G1–G6. Garmin is lightly used here and its current
behaviour is not trusted, so this step also gets a playback tape that
covers each phase.

This step builds the two shared pieces:

- the owed query as a helper over a listing table and a held stamp;
- **the interruption test** (below).

**Step 2 — Slack.** The test of ranges: coverage per channel, deletion
by absence per page, threads owed by newest-reply stamp, attachment
bytes owed by edge. Closes S1 and S3–S7. The refresh window still
measures from the wall clock; it should take the run's pinned now.

**Step 3 — email.** Both deltas become "list, then advance the token".
Closes E2–E9.

**Step 4 — Notion.** Closes N1–N4, N6, N7.

**Step 5 — the rest of the network sources** (ChatGPT, Claude, GitHub
and GitLab, DAV, AirVisual, YoLink), then delete `scope_config`, lint
check 8 and the AGENTS.md section.

**Step 6 — local sources** move to one whole pass per run, after timing
the large inputs. `file_checkpoint` and the three copies of the
overlapping-snapshot protocol go.

## 8. The interruption test

These bugs were found by reading, and none had a test that could have
caught it: an interruption at one particular request is hard to arrange
by hand, and there are thousands of places to interrupt. The rule in §2
makes one test possible for every provider:

> Run a download against its playback tape to the end, and keep the
> store. Then, for each request *k* the run makes: run again from an
> empty store, fail the run at request *k*, seal whatever is there, and
> run once more to the end. **The final store must equal the
> uninterrupted one.**

It is the download's counterpart of `render_contract_test`, which
asserts that an incremental render equals a cold one. Sealing at the
failure is deliberate: under §2 every transaction boundary is a correct
store, so the test may seal anywhere. A provider still relying on "the
unsealed tail is discarded" fails it, which is the point. A second leg
stops the run at request *k* through the stop flag, since a stop takes
different paths from an error.

The playback layer already serves requests from a tape and can already
make one fail (`github/tests/github_tests/run_problems.rs` does this by
hand). The harness needs a way to fail the *k*-th, whichever it is.

When a provider passes this test, sealing a failed run becomes safe for
it, and the behaviour #1008 tried and withdrew can come back.
