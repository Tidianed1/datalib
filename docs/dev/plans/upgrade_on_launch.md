# Upgrade on launch: migrate the raw stores, then offer a re-render

**Status: proposed 2026-10-08, being built on
`claude/upgrade-multi-source-sync-a34fe8`.** Checked against the tree at
`5cb634969`.

## 1. The problem

A new build can change the layout ("shape") of two kinds of store:

- **A raw store** — what a download writes. A change there reaches an
  existing store through the provider's migration ladder
  (`etl/README.md` §"The migration ladder"), which runs when the store
  is opened to write. Only the source's own ingest step opens it to
  write, so a raw store stays in the old shape until that source syncs.
- **A render store, or the grid index** — derived stores. A change
  there is a rebuild: the render step runs again over the whole raw
  store.

Today the runner reconciles this lazily, inside whatever sync the
person pressed. A store in an old shape is pulled into the scope of any
request that reaches a step reading it (`dag/README.md` §"What the loop
runs"), so pressing Sync on one source after an upgrade re-renders
every other source too. And because the raw stores are not migrated
until their own source syncs, those pulled-in renders read raw stores
in the old shape. #1053 renamed a column on the shared ladder and every
such render failed on it; #1055 papered over that one rename in
`datalib_step/src/render.rs` (`fetch_problems_of` reads the old column
name). The next non-additive raw change would need another such patch.

## 2. The design

**On launch, before the loop takes any request, every raw store a
newer build has not yet opened is migrated, with no download.** Then,
if any render store or the index is in an old shape, the app asks
whether to re-render now.

1. **Which stores.** A built-in ingest step whose raw store exists and
   whose `_datalib_meta` names another build (`datalib_version` or
   `git_hash` differs from this one). Opening a store rewrites those
   rows, so a store is migrated at most once per build. A store a
   *newer* build wrote never gets here: `inspect_root` refuses the root
   first (`newer_root`).
2. **The migrate pass.** The runner invokes each such step with
   `DATALIB_DAG_MIGRATE=store`, the way `--reset` invokes it with
   `DATALIB_DAG_RESET`. `datalib-step` then calls the provider's
   `processor::migrate(raw_dir)`, which opens the raw store with its
   ladder and closes it. Opening already climbs both ladders, applies
   additive DDL, converts an old blob store and seals each step to
   `main` (`doltlite_raw::open_inner`), so there is nothing else to do.
   It reads no config and reaches no network or file: the function
   takes only the store's directory. The store's new head is recorded
   as the step's version, so what reads it is stale; the step's last
   success is kept.
3. **Where it runs.** In the host, after `take_over` and before the
   first idle turn (`http/src/supervisor.rs::host`), and at the start
   of a `datalib-dag` run. The runner lock is held, so no other writer
   is open. A store the pass cannot migrate (a non-additive change
   with no rung: `SchemaBreak`) is a failed run on that step's row,
   naming the way out (a rung, or Reset); the pass goes on to the
   next store.
4. **The UI.** While the pass runs, `/api/config` says so
   (`upgrade.migrating`), and `App.vue` shows a blocking view listing
   the sources and how each went.
5. **The re-render offer.** `/api/config` lists the writers whose store
   is in an old shape (`upgrade.rerender`: the render steps and the
   index, from the record, by the rule `tick::in_old_shape` already
   states). When it is not empty the UI asks "Your rendered documents
   are out of date. Re-render now?". Yes opens one request per group
   rooted at those steps, `opened_by = "upgrade"`. No leaves them as
   they are for this session; the next launch asks again.
6. **No more pulling in.** `tick::scope` stops adding old-shape writers
   to a request. A sync of one source re-renders that source only. The
   index reads every other render store as it is; one in another shape
   it leaves as the index had it, with a warning (`grid_index`'s
   existing guard). Rule 6's hold — a reader waits for a running writer
   of an old-shape store — stays, since it costs nothing and saves an
   index pass over a store about to change.
7. **No more reading old raw shapes.** The `render.rs` fallback for the
   pre-#1053 column name goes, with its test. A render reads the
   current raw shape or fails loudly.

## 3. Tests

The schema-change machinery has unit tests per rung and nothing that
checks the pieces together. In order of what they would have caught:

1. **Every source type can be migrated** (`datalib_step`): for each
   `SourceType`, `migrate` on a missing store creates nothing; on a
   store at the current shape makes no commit; on a store created at
   the shape of each *past release* ends at the current shape. The past
   shapes are checked in: a test snapshots every source type's raw DDL
   and ladder height, and a release copies that snapshot to
   `schema_history/<version>/`. A non-additive DDL change with no rung
   then fails in the PR that makes it, for every provider, without an
   old binary.
2. **The tick and the record** (`dag`): pure tests that a request no
   longer pulls in an old-shape writer, that `old_shape_writers` names
   exactly the writers `in_old_shape` says, and that a migrate
   invocation keeps the step's last success and records its version.
3. **The host** (`dag`, fake step script, like `subprocess.rs`'s reset
   test): on a root whose stores name an older build, the pass runs
   before a request opened at boot is taken on; on a root at this
   build, it runs nothing.
4. **The app** (`http`): `/api/config` reports the pass and the
   re-render list; a boot on an older root migrates before it serves a
   sync.
5. **The UI** (Playwright): a root forged to an older build shows the
   blocking view and then the re-render question; Yes re-renders.
6. **Later, a real upgrade** (separate PR): a release publishes the TNG
   fixture's raw stores as an asset; the next build's test fetches the
   previous release's by sha256, migrates and renders them, and checks
   (a) the render equals a fresh one's and (b) a download against
   HTTP playback afterwards owes nothing. (b) is the check that would
   catch a rung that leaves every record owed.
