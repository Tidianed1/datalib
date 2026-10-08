# Contact editing: what is still to build

*Proposal (2026-10-08). Nothing here is built. The contacts app's
store, its routes and how chips read it are in
[`../contacts.md`](../contacts.md); which handles are one person, and
how a person says so, is the other plan,
[`contact_linking.md`](contact_linking.md). The doltlite behaviour this
rests on is pinned by tests and written up in
[`../doltlite.md`](../doltlite.md) §"Merging a branch" and §"Reverting
a commit"; check a fact there before relying on it.*

The contacts app holds what a person says about the people in their
mirror. Today it holds a name, a note, a photo and the linked handles,
and the only way to change them is the chip's popover: one operation,
one commit. This plan is the **contact card**, where a person edits a
contact at length, copies fields from what each source says about that
person, and later publishes the result to a CardDAV address book.

It is the first place in datalib where a person edits data rather than
mirrors it, so it also sets the pattern any later editable record
follows: drafts, saving, seeing someone else's change, undo.

## Words

[`../contacts.md`](../contacts.md) §"Words" holds. Also:

| word | means |
|---|---|
| **draft** | a contact's unsaved edits: a branch of the store holding them as uncommitted rows. |
| **save** | merging a draft into the store's published state, as one commit. |
| **published** | what readers see: the store's `main`, which the applet moves only when it seals. Chips, search and every other card read this. |
| **source value** | what one source's account of the person says a field is (a Slack profile's title, an address-book card's birthday). |

## What a contact holds

Today: `contacts` (name, note), `photos`, and the handles linked to it.
To copy fields from sources the card needs somewhere to put them, so
the store gains a table shaped like vCard's properties:

```
fields(contact_id, field_id, kind, label, value, position,
       copied_from_source, copied_from_handle, copied_at_utc, tz_offset)
```

- `kind` is a closed set, an enum: organization, title, birthday,
  address, url, and others as the card needs them. Handles stay
  handles; an email or a number is linked, not stored as a field.
- `label` is free text a person types ("home", "work").
- `copied_from_*` says which source's account the value came from, or
  is empty for a value the person typed.

**A copied value is a copy.** A later change upstream does not flow in
by itself. The card compares each copied value with the source's
current one and marks a field whose source now says something else
("Slack now says *Commander*"), with one click to take it.

## Copying from sources

Beside the contact's own fields the card shows each linked handle's
source accounts, which `/people` already serves from the index. Each
source value has a copy button, and each account has "copy all". Where
two sources disagree the person picks one; nothing is merged
automatically. This is the "merge data from several sources" step: the
person does it field by field, and the store records where each value
came from.

## Drafts

- **One draft per contact**, a branch `draft/<contact_id>` cut from the
  published commit when the card starts editing. Two windows open on
  the same contact edit the same draft and see each other's autosaves.
- **Autosave is a plain SQL write to the draft branch**, made when
  typing pauses or a field loses focus, and left uncommitted. Doltlite
  keeps a branch's uncommitted rows in the file, past the connection
  that wrote them and past a reset of another branch.
- **Nobody else sees a draft.** Readers read the published commit.
- **Reopening the card resumes the draft**, from any window, after a
  reload or a restart.
- **Discard deletes the branch** (`dolt_branch('-D', …)`), uncommitted
  rows and all.
- **Every draft operation runs in the `datalib_contacts` applet**, the
  store's one writer: cutting the branch, autosaving, the stale check,
  saving, deleting the branch. A process that moves a ref is a writer
  (AGENTS.md § "Doltlite"). The applet changes branch per request with
  `dolt_connect_branch`, which writes nothing, and ends each request
  back on its writer branch.

## Saving

One transaction in the applet, on its writer branch:

1. Commit the draft's uncommitted rows on the draft branch. A merge
   takes a branch's commits, not its uncommitted rows.
2. `BEGIN`, then `dolt_merge('--squash', 'draft/<id>')`. A squash
   lands as one commit with one parent, so the published history has
   one commit per save, and undo works per save.
3. Where the draft and the published state changed the same cell,
   take the draft's side: the person saving is the last writer. Doltlite
   calls the merged-in branch "theirs", so this is
   `dolt_conflicts_resolve('--theirs', …)`; in our code it is named
   for what it means, the draft wins. Cells only one side changed
   merge cleanly, so a rename in the card and a note changed elsewhere
   both survive.
4. Seal (`commit_run`), which publishes, and delete the draft branch.

**A person never overwrites a change they have not seen.** The save
request carries the published commit the card last showed. If the
published state has moved since and the move touched this contact, the
applet saves nothing and answers with the new state (the `If-Match` /
`412 Precondition Failed` pattern of HTTP), and the card shows the
change as below. Saving again then knowingly keeps the draft's values.

Not yet pinned: a squash merge that conflicts inside a transaction.
The facts cover a conflict resolved in a plain merge and a squash
without one; this needs its own fact before the save is built.

## Seeing another writer's change

**The push.** `datalib-http` already watches the data root and pushes
payload-free "ask again" frames over one SSE stream
(`http/src/watch.rs`, `ui/src/live.ts`); the grid index's frame fires
only when its head commit moves. The contacts store gets the same: a
`LiveTable` variant (both halves by hand) sent when the published head
moves. On it `people.revalidate()` redraws every chip, and an open card
refetches.

**A card being viewed** redraws in place.

**A card being edited** compares, for each field, three values: what
it was where the draft was cut (`dolt_merge_base`), what the draft has
now (a diff to `'WORKING'`), and what is published now. A pure function
of those three decides each field:

| draft changed it | published changed it | the card |
|---|---|---|
| no | no | unchanged |
| no | yes | shows the published value; saving keeps it |
| yes | no | shows the draft's value |
| yes | yes, to something else | marks it: "changed elsewhere to *X*", with *use theirs* and *keep mine* |

That is the same three-way merge the save does, shown before it
happens. The card cannot pull the published change into the draft
itself: doltlite refuses a merge into a branch with uncommitted rows.

A link made in the popover or by an agent while the card is open
changes `handles`, not the draft, and shows on the card at once.

## Undo

Each save is one commit, so undo is `dolt_revert` of it: a new commit
that puts back what the save changed, leaving later commits alone.
Doltlite refuses it when a later commit changed the same rows, and the
card says so rather than guessing. A contact's history is the commits
that touched its rows. A draft of the store's `history` and `revert`
and their routes is on the branch `claude/contact-card-wip`; its
doltlite facts have landed.

## The export

JSON of every table, and a vCard per person leaving out handles that
stopped working, so what a person wrote is readable without doltlite.
The vCard writer is also what publishing needs.

## Publishing to CardDAV (later)

A contact can be written to a CardDAV address book. CardDAV versions
each card with an ETag and takes `If-Match` on a write, so publishing
uses the same rule as saving: a card changed upstream since we last
read it is refused, re-read, shown as above, and written again. It
needs a writable account, chosen on purpose: the `contacts` provider
only ever reads.

## Routes to add to the applet

| route | does |
|---|---|
| `POST contact/{id}/draft` | cut the draft, or return the one there is |
| `PATCH contact/{id}/draft` | autosave: set fields on the draft |
| `GET contact/{id}/draft` | the base, draft and published values, per field |
| `POST contact/{id}/draft/save` | save, carrying the published commit the card showed |
| `DELETE contact/{id}/draft` | discard |
| `GET contact/{id}/history`, `POST revert` | a contact's saves; undo one |
| `GET export` | the JSON and vCard export |

## Open questions

- **Are links part of the draft?** Proposed: no. A link changes who a
  chip names everywhere, so it stays an immediate operation, as in
  the popover, and the card's draft holds only what the contact says.
- **When does an abandoned draft go?** A list of drafts, and an expiry,
  or neither until drafts pile up.

## Order of work

1. **The facts still missing**: a squash merge that conflicts inside a
   transaction; reading the published state from a draft connection.
2. **The store**: the `fields` table (a ladder rung), drafts, save,
   history and revert.
3. **The applet's routes and the live frame.**
4. **The card**: viewing; editing with drafts and the three-way marks;
   copying from sources; the chip's double-click opening it
   (`onChipDblClick` in `ChatBody.ce.vue`, `onDblClick` in
   `GridCard.ce.vue`; the card's source builder goes in
   `cardSources.ts`). An e2e spec on the `contacts` project's root.
5. **The export.**
6. **Publishing to CardDAV.**
