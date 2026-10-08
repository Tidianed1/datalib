# Progressive search: the index first, then keywords, then everything

*Proposal (2026-10-08). Nothing here is built. Every number below was
read from a real data root on that day (122,487 `grid_rows`, a 1.6 GB
qmd index) through its `system/runs/runs.sqlite`, the
`//third-party/doltlite:doltlite` shell and the qmd 2.8.3 CLI the app
ships.*

Search shows nothing until its slowest part, a qmd hybrid search, has
finished. A pasted identifier that the grid index could find in
milliseconds waits for that search too. This plan answers a search in
up to three passes, fastest first. Each pass adds rows below the ones
already shown, and nothing already shown moves.

## What is slow, measured

**A pasted uuid took 17.9 s.** It was the `markdown_uuid` of one
document, searched as bare text. The request's log lines:

| step | cost |
|---|---|
| `qmd mcp` was not running, so the search started it | ~2 s |
| qmd's hybrid search (a keyword search and a vector search, merged; rerank is off) | 13.0 s |
| mapping qmd's hits to grid rows: `grid_row_refs` reads `uuid, kind, qmd_path, provider, is_document` for every row | 1.9 s |

SQL would have answered it at once. The uuid is one row's `uuid` (the
primary key) and six rows' `conversation_uuid` and `markdown_uuid`,
and all three columns are indexed. The three lookups took 37 ms in
all, process start included. The search bar cannot reach them,
because a bare term is always free text, and free text always goes
to qmd. `uuid` and `markdown_uuid` are not search keys at all.
`convo:` is a key, but a person pasting an id does not know that.

**The passes qmd could run cost very different amounts.**

| search | cost |
|---|---|
| keyword (BM25) only, `qmd search`, a fresh process each time | 0.11–1.26 s |
| hybrid, warm daemon (124 searches) | median 2.7 s, p90 3.0 s, max 3.8 s |
| hybrid, the first search or two after the daemon starts (48 searches) | median 4.1 s, p90 4.8 s, max 13.0 s |
| vector only, `qmd vsearch`, a fresh process, while an `embed` ran | 45.8 s |

The keyword search found the uuid's document by itself: its
frontmatter carries the uuid.

**The daemon restarts far more often than it needs to.** It started
48 times for 172 searches. `QmdDaemon` restarts `qmd mcp` whenever
`index.sqlite`'s mtime has moved since it started
(`unified_index/src/qmd/daemon.rs`, `ensure_started`). Each embed
batch moves that mtime, so while a source embeds, every search pays
for a restart and a model load. [`qmd_behaviour.md`](../qmd_behaviour.md)
already notes this, and leaves open whether a long-lived `qmd mcp` sees
rows committed after it started.

**Mapping hits to rows reads the whole table, on every qmd search.**
`grid_row_refs` (`unified_index/src/dolt_repo.rs`) reads all 122,487
rows to build the path→row map, then uses it for at most
`QMD_DEPTH` (1,000) hits. sqlx logged it slow (over 1 s) twice, at 1.5
s on average. A faster run passes the threshold silently, so how much
it usually costs is not known.

## The shape: three passes, each a longer list

| pass | what answers it | needs qmd | needs a model |
|---|---|---|---|
| `index` | SQL lookups for the terms that look like identifiers | no | no |
| `keyword` | qmd's lex sub-query alone | yes | no (to verify, below) |
| `full` | qmd's lex + vec, what runs today | yes | yes |

**Each pass's list starts with the previous pass's list, unchanged.**
The `keyword` list is the `index` list followed by BM25's hits that are
not already in it. The `full` list is the `keyword` list followed by
the hybrid hits not already in it. So:

- **Nothing needs reranking.** No pass mixes scores from different
  passes, which would mean nothing anyway: a BM25 score and a hybrid
  score are not on one scale, and an exact identifier hit has no score.
  An exact identifier hit goes first because it is the record the
  person named, which no text match outranks. A row the keyword pass
  found keeps its place even where hybrid would have ranked it lower.
  A slightly worse order costs less than rows that move under the
  reader.
- **Paging stays valid across passes.** An offset into the `keyword`
  list points at the same row in the `full` list, so a person who has
  scrolled keeps their place when the next pass lands, and the
  `through` the grid already sends still means what it did.
- **The results cache keeps working.** The pass becomes part of
  `results::Key` (`applets/src/unified_index/results.rs`). Each pass
  reads the previous pass's list from the cache and extends it, so a
  later pass never repeats an earlier one's work.

**With an explicit sort** (Date, Author), the list is in the sort's
order, not rank order. A later pass can then put rows between ones
already shown. The grid already handles rows appearing in the middle:
it merges a live refresh's rows the same way (`showChanged`). This
case does not need to keep the prefix property.

**When a pass is skipped:**

- When every term is an identifier, `index` is the whole answer. qmd's
  hits for a bare uuid are the documents whose text contains it, which
  is the same document again. A uuid search then never waits on qmd.
- `qmd_vsearch:` asks for meaning, not words, so it skips `keyword`.
- With no free text, nothing changes: the search is SQL already.

### The `index` pass: what an identifier is

A pure function reads each bare free-text term and names the columns
it could be a value of. A term it recognizes becomes a lookup (`col =
?` on each column it names, OR'd together), not text for qmd.

| looks like | columns |
|---|---|
| a uuid, or a slug ending in one (`extract_uuid_suffix` already parses both) | `uuid`, `conversation_uuid`, `markdown_uuid`, `notion_page_uuid` |
| a handle `datalib_handle` parses: an email address, a phone number, `slack:T…/U…` | `author_handle` |
| later, if wanted: an upstream URL (`chatgpt.com/c/…`, a Slack permalink) | `upstream_id` |

Every column here needs an index, and
`every_filter_key_is_served_by_an_index` keeps it that way.
`grid_rows_by_markdown` exists (created in
`etl/render/src/grid_index.rs`) but is not declared on `GridRow`; it
moves there. Within the pass the order is: the row whose `uuid` it is,
then the document it is in, then the rest in the grid's default order.

**A query that mixes an identifier with words** (`someone@example.com
budget`) runs all three passes. The identifier's rows lead, and the
words go to qmd without it. The other reading treats the identifier as
a filter on the words' hits ("budget, involving this person"). That is
closer to what a person may mean, but it changes what the query
language says, so it is left until the tiers have been used for a
while.

### Delivery: one request per pass

The grid asks for `pass=index`, shows the rows, then asks for
`pass=keyword`, then `pass=full`. Each answer carries the whole list's
`total` and whether a later pass is still to come. In place of the
spinner, the card says "searching text…" while a pass is outstanding.
The groups endpoint takes the same parameter.

A single streamed response (SSE or NDJSON) was considered and is not
proposed. Separate requests reuse the gateway, `fetchRows`, the cache
and the paging as they are, and a pass that fails or times out is one
failed request: the rows of the earlier passes stay on screen, with
the failure beside them. Today a qmd timeout fails the whole search.

## Making each pass fast in itself

These matter whether or not the passes land, and each is a small,
separate change.

1. **Map only the hits.** Look up `WHERE qmd_path IN (…)` for the at
   most 1,000 paths qmd returned, instead of reading every row. That
   needs an index on `qmd_path`. The alternative is to keep the map per
   index commit. Measure both on a real root; the lookup is simpler if
   it is fast.
2. **Stop restarting the daemon on every mtime move.** First measure
   the open question in `qmd_behaviour.md`: does a live `qmd mcp` see
   documents and vectors committed after it started? If it does, the
   restart can go. If it does not, restart on the qmd index's commit
   (what `keyword_index` and `embed` finish with), not on its mtime.
3. **Confirm the keyword pass loads no model.** Send `qmd mcp` a
   `query` whose `searches` hold only a `lex` sub-query, and check from
   its timing that it never loads the embedding model. If it does load
   it, the `keyword` pass reads qmd's `documents_fts` table directly,
   which is how the CLI's `qmd search` answers in 0.1 s.
4. **Expect the vector search to lose to a running embed.** During an
   embed, `full` can run past `QMD_ANSWER_DEADLINE` (20 s). With passes,
   that costs the `full` pass's rows and a note that the embed is
   running, not the whole answer.

## Order of work

1. **The paste.** The uuid recognizer, the `index` pass as the whole
   answer when every term is an identifier, and no qmd. Test: a
   uuid-only search answers with its rows without asking qmd at all
   (the applet tests can give it a daemon that fails on any request).
   This needs no passes in the API.
2. **Map only the hits** (above, 1).
3. **The daemon restart** (above, 2), after the measurement.
4. **Passes in the API and the grid**: the `pass` parameter, the cache
   key, the grid asking pass after pass, and the "searching text…"
   state. An e2e spec that holds the `full` answer back (`page.route`)
   checks that the `index` and `keyword` rows are on screen while it
   waits.
5. **Handles**, then any other identifier kinds that prove useful.

## Open questions

- Is `keyword` worth a round trip once the daemon stays warm? A warm
  hybrid search takes 2.7 s, almost all of it the vector half, so
  probably yes. Measure after step 3.
- Should a pasted URL from a source (`chatgpt.com/c/…`) find its
  conversation? It needs `upstream_id` indexed. That index costs every
  write, so it is only worth it if people actually paste URLs.
