# Search tabs: fields first, then words, then meaning

*Proposal (2026-10-08). Nothing here is built. Every number below was
read from a real data root that day (122,487 `grid_rows`; a 1.6 GB
qmd index holding 180,773 vectors for 60,559 documents in 8 sources)
through its `system/runs/runs.sqlite`, the
`//third-party/doltlite:doltlite` shell, and the qmd 2.8.3 the app
ships, run on a copy of the index. Each SQL timing includes about
0.05 s of process start.*

Today a free-text search is one qmd hybrid query (keyword and vector,
merged), and the grid shows nothing until it answers. This plan runs
three searches at once, each shown in its own tab: a SQL match on the
grid's own fields, qmd's keyword search, and qmd's vector search. A tab
is greyed out until its search answers, and what a tab shows never
changes while you look at it.

## What is slow, measured

**A pasted uuid took 17.9 s.** The request's log lines: the search
started `qmd mcp` (~2 s), qmd's hybrid search took 13.0 s, and mapping
its hits to grid rows took 1.9 s more (`grid_row_refs` reads every
row's `uuid, kind, qmd_path, provider, is_document`). SQL finds the same
rows in 0.03 s: the uuid is one row's `uuid` and six rows'
`conversation_uuid` and `markdown_uuid`, all indexed. Neither `uuid`
nor `markdown_uuid` is a search key, so the search bar cannot ask.

**Inside a hybrid search, the vector half is the cost.** Query expansion
never runs (the daemon sends typed `lex` and `vec` sub-queries), and
rerank is off.

| step | cost |
|---|---|
| loading the embedding model, once per `qmd mcp` process | ~1.5 s |
| embedding the query | 0.09 s |
| keyword (BM25) query | 0.07–0.5 s |
| vector query, every source | 5.3–5.8 s |
| vector query, one source | 0.7–1.2 s |

A vector query that names no collections searches each of qmd's
default collections in turn (`searchVec` in qmd's `dist/store.js`), and
for a collection over 20,000 vectors it scans all 180,773. Two sources
here are that big, so an unscoped vector query scans the whole table
twice, plus the smaller sources one by one.

**A free-text search returns at most 40 documents.** qmd fuses the
`lex` and `vec` lists and cuts the result to `candidateLimit`, which
defaults to 40, rerank or no rerank. The daemon asks for 1,000
(`QMD_DEPTH`) but never passes `candidateLimit`, and every search in the
log came back with 40 hits.

**The daemon restarts far more than it needs to.** It started 48 times
for 172 searches, because it restarts whenever `index.sqlite`'s mtime
moves, and every keyword or embed batch moves it. A running `qmd mcp`
reads the index live, so new rows need no restart
([`qmd_behaviour.md`](../qmd_behaviour.md), "How a running `qmd mcp`
behaves").

**SQL is fast enough to go first.** On the same grid index, with the
indexes it already has:

| match | time |
|---|---|
| a uuid against `uuid`, `conversation_uuid`, `markdown_uuid` and `notion_page_uuid` (one index lookup each: `MULTI-INDEX OR`) | ~0.03 s |
| an exact handle, `author_handle = 'email:…'` | 0.03 s |
| a word as a substring of `conversation_name`, `author`, `author_handle`, `channel` or `account` | 0.38–0.71 s |
| a word as a substring of `preview` | 0.31 s |

## The shape: three tabs, three searches

| tab | what answers it | pages |
|---|---|---|
| **Fields** | SQL over the grid index: identifiers matched exactly, words matched as substrings of the short fields | without limit, like every SQL search |
| **Words** | qmd's keyword (BM25) query over every document's whole text | qmd's ranked list |
| **Meaning** ("QMD semantic (vector)") | qmd's vector query | qmd's ranked list |

The three run at once. Each tab reads its own list from the results
cache (`results::Key` gains the tab), with its own count, paging, sort
and grouping. **The tabs answer in different ways, and the labels say
so:** Fields finds what is written in a row's fields, Words finds the
words anywhere in a document, ranked, and Meaning finds documents about
the same thing, whatever words they use.

**Nothing changes under the mouse.** A tab is greyed, with a spinner,
until its search answers; then it shows its count. Rows appear in a
tab only by your choice: clicking a tab is the only thing that changes
the rows on screen.

**Which tab opens.** Fields answers first. If it has rows, it opens.
If it has none, nothing is painted yet, so the first of Words and
Meaning to answer with rows opens, and nothing moves to make way. Once
any rows are on screen, the open tab never changes by itself.

**No fusion.** Hybrid search merged the keyword and vector lists into
one; with a tab each there is nothing to merge. `qmd_vsearch:`, which a
person may have typed or saved, opens the Meaning tab. `qmd:` opens
Words. The "Meaning only" checkbox goes: its job is now a tab.

**A query of identifiers only skips qmd.** A pasted uuid or email
address is answered by Fields, and the qmd tabs say "not searched: an
identifier". qmd's hits for a uuid are the documents whose text
contains it, which Fields already found.

### How deep each tab goes

Fields pages without limit through the grid's existing SQL paging.

Words and Meaning are ranked lists, so each is cut somewhere. Today the
cut is 40, by accident. The daemon passes `candidateLimit`, and each
tab gets its own depth: Words deep (a keyword match is either there or
not, so its long tail is still real matches), Meaning shallower (past a
few hundred, nearness is noise). Measure what a larger
`candidateLimit` costs before choosing the numbers; the vector scan
already fetches `limit × 3` candidates per collection, so most of the
cost is paid either way.

### Identifiers: which columns, and how

A pure function reads each bare term and says which columns it could
be a value of. A term it recognizes becomes an exact match in Fields,
not a substring.

| looks like | matched against |
|---|---|
| a uuid, or a slug ending in one (`extract_uuid_suffix` parses both) | `uuid`, `conversation_uuid`, `markdown_uuid`, `notion_page_uuid`: one `OR`, one index lookup each |
| a handle `datalib_handle` parses: an email address, a phone number, `slack:T…/U…` | `grid_row_handles` (below) |

**Every person a row names, in one table.** `grid_rows` holds one
`author_handle`. An email's To, Cc and Bcc, a chat's participants and
mentions are only in each provider's raw store and in the rendered
HTML. A column per role would mean a column, an index and a search
clause for each, in every provider. Instead, the grid index gets one
narrow table:

```
grid_row_handles (handle, role, uuid)   index (handle, uuid)
```

A render writes one row per person a grid row names (`from`, `to`,
`cc`, `bcc`, `participant`, `mention`), the way it writes `edges`, and
`grid_index` copies them. Finding a handle is then one indexed query,
`uuid IN (SELECT uuid FROM grid_row_handles WHERE handle = ?)`, the
same for every provider and every role. A new role or a new provider
costs a render change and no search code. The `role` column is there
for a later `to:` or `cc:` key, which is one more `AND role = ?`.

## Making qmd's tabs fast in themselves

1. **Name the collections on every request.** An unscoped query
   searches the collections the server read at startup (fact 3 in
   `qmd_behaviour.md`), so the daemon sends the configured list.
   `source_id:` already scopes a query to its source's collection,
   which is what makes a scoped vector query take under a second.
2. **Restart only when the index file is replaced.** Watch its inode,
   not its mtime; a new collection is covered by step 1.
3. **Map only the hits.** Look up `WHERE qmd_path IN (…)` for the paths
   qmd returned, instead of reading every row. That needs an index on
   `qmd_path`. The alternative is to keep the map per index commit.
   Measure both.
4. **The vector scan itself.** Each big source is a scan of every
   vector. Splitting the vectors per collection, or a smaller vector
   type, is a change in qmd, not here; note it upstream and measure
   again after 1 to 3.

## Order of work

1. **Fields, for identifiers.** The recognizer, the uuid `OR`, and no
   qmd for an identifier-only query. Test: a uuid search answers
   without asking qmd at all (the applet tests can give it a daemon that
   fails on any request).
2. **The qmd fixes** (above, 1 to 3) and `candidateLimit`, each its own
   PR. Fact tests in `qmd_facts_test` for anything new we rely on.
3. **Tabs.** The `tab` parameter on the search and groups endpoints,
   the cache key, three requests from the grid, the tab strip with its
   greyed state and counts, the opening rule. Fields matches words as
   substrings here. The e2e spec holds the qmd answers back
   (`page.route`) and checks that Fields is on screen, that the qmd tabs
   are greyed, and that their answers arriving changes no row.
4. **`grid_row_handles`.** The table, `grid_index` copying it, and the
   email renders filling it first, then the chat ones.
5. **More identifier kinds** if wanted: an upstream URL (`chatgpt.com/c/…`)
   against `upstream_id`, which needs its own index.

## Open questions

- Whether Fields should match words as substrings of `preview`. It is a
  cut of the text, so a match there is a match in the first few hundred
  characters only, which may confuse more than it helps next to Words.
- A query that mixes an identifier and words (`someone@example.com
  budget`): Fields can treat the identifier as a filter and the words
  as substrings, which is likely what a person means. The qmd tabs
  would search the words alone.
- The live refresh when the index commits still patches rows in place
  (`applyPatch` in `GridCard.ce.vue`). The same "nothing changes
  unasked" rule may want a "N changed · Show" there too; that is its
  own change.
