# Contacts: who a handle is

How datalib knows that the `riker@enterprise.org` in a mail, the
`+1 202 555 0101` in a WhatsApp chat and the `U_RIKER` in a Slack
thread are the same person, and draws them as one chip. Three layers,
each one of which works without the one above it:

1. **Handles.** A render writes every person it can identify as a
   normalized identifier, a *handle*, inside the document. The index
   knows handles and never contacts.
2. **Accounts.** Every source that knows something about a person says
   so in one shape, `DatalibContact`, and the index keeps those rows so
   it can answer "who holds this handle?" from the sources alone.
3. **Contacts.** A person links handles to a contact of their own in
   the contacts app, the one store under a data root that nothing can
   rebuild. Its answer ranks above every source's.

What is still to build: [`plans/contacts.md`](plans/contacts.md)
(managing contacts, `row_handles`, contacts in search) and
[`plans/chips.md`](plans/chips.md) (chips for groups, steps and system
events). This page says what the tree does.

## Words

| word | means |
|---|---|
| **handle** | one identifier in one namespace, normalized: `email:riker@enterprise.org`, `tel:+12025550101`, `slack:T01/U02`, `signal_aci:<uuid>`. `datalib_handle` makes them; nothing else spells one. |
| **chip link** | how a document names a person: a markdown link whose href is the handle as a URI, `[Will Riker](mailto:riker@enterprise.org "Will Riker <riker@enterprise.org>")`. The viewer draws it as a chip. |
| **account** | a `DatalibContact`: one source's description of a person, keyed by `source_id` and that source's own `key`. A Slack profile, an address-book card, a LinkedIn connection, or what a chat saw of an author. |
| **contact** | a person's own record in the contacts app, also answered as a `DatalibContact` with `source_id = "datalib_contacts"`. |
| **link** | a row in the contacts app saying a handle belongs to a contact. Only a person makes one. |
| **the contacts app** | the `datalib_contacts` applet and its store under `datalib_curated/`. Optional: without it, chips still say what the sources know and offer nothing to link. |

## Handles

[`datalib/backend/handle`](../../datalib/backend/handle/src/lib.rs) is a
crate with no first-party dependencies, so render crates, the index, the
applets and the contacts store all share one definition. A handle is
`<kind>:<value>`. The rule that matters most: **where a native id is an
email address or a phone number, the handle is `email:` or `tel:`, not a
per-app kind**, so one link covers every app that reaches a person that
way. Only an id that is opaque by nature gets a kind of its own.

| kind | value | made by | URI (`Handle::to_uri`) |
|---|---|---|---|
| `email` | the address, lowercased | `Handle::email` (takes a `mailto:` too) | `mailto:<address>` |
| `tel` | E.164, `+` and digits | `Handle::tel` (any spelling with a country code; `Handle::whatsapp_jid` for a `<number>@s.whatsapp.net`) | `tel:<number>` |
| `slack` | `<team_id>/<user_id>` | `Handle::slack` | `slack://user?team=<team>&id=<user>` |
| `signal_aci` | a Signal account id, a lowercase dashed UUID | `Handle::signal_aci` | `datalib:handle/signal_aci/<uuid>` |

`Handle::parse` reads back exactly what `as_str` wrote and refuses any
other spelling, which is what every store and every wire format goes
through (serde uses it). `Handle::rebuild` is looser: it runs a stored
value through its kind's constructor again, which is how a store brings
its handles along when the rules change. `Handle::from_uri` reads a
chip link's href; `Handle::describe` is the link's title and the text a
chip copies as, `Will Riker <riker@enterprise.org>`,
`Data (slack:T01/U02)`.

A number without its country code is not a handle. The value is kept
as the source wrote it (`ContactHandle::value` with `handle: None`), so
nothing is lost, but nothing links to it either.

### Changing the rules

`RULES_VERSION` in the handle crate is bumped whenever a constructor
returns something different for some input: a spelling newly accepted
or refused, a value normalized another way. Two things hang off it:

- **Every source renders again.** The render step puts the version in
  every source's render params as `_handle_rules`
  (`datalib_step/src/render.rs`), so a bump re-renders everything the
  way any param change does, and no provider bumps its own
  `RENDER_VERSION` for it.
- **The contacts store respells the links a person made.** Each rules
  change adds a rung to `datalib_contacts::LADDER` that runs
  `rebuild_handles`, and a test fails until it is added. A link the
  new rules cannot read, or whose new spelling another contact holds,
  is kept as written and logged, never dropped.

**A new kind is not a rules change**: no stored handle reads
differently, so the version stays. It does touch six places, all of
which the build or a test checks except the last:

| where | what |
|---|---|
| `handle/src/lib.rs` | the `HandleKind` variant, its constructor, and an arm each in `rebuild`, `to_uri` and `describe`; a row in `every_kind_round_trips_through_its_uri` |
| `contact_schema/src/lib.rs` | `Medium::of_kind` |
| `applets/src/unified_index/columns.rs` | `handle_mark`, the mark the grid's Author chip shows |
| `ui/src/cards/chipLinks.js` | `KINDS`, `handleFromUri`, `uriFromHandle`, and a row in `tests/chip_links.test.ts` |
| `ui/src/cards/contacts.ts` | the `HandleKind` union, `handleKind` and `KIND_ICON` |
| `tests/fixtures/ingested_tng_test.py` | a fixture person reached by the new kind, so the index is seen to know them |

### Who writes a handle

A provider's render decides, at normalize time, which identifier it
has for a person; `NormalizedChatItem::author_handle`,
`Recipient::handle` and `NormalizedReaction::reactor_handle` carry it.
What each source has today:

| source | author | more |
|---|---|---|
| email | the From address | To and Cc as recipients |
| Slack | `slack:<team>/<user>` | a `<@U…>` mention in a body; each reaction's user; the profile's email, in the account |
| WhatsApp | the sender's number, a linked id (`…@lid`) through `jid_map` | each reaction's sender |
| Signal | the number, else the account id (ACI); a recipient known by PNI alone has none | number and ACI together, in the account; one known by ACI alone reads as the dashed ACI. The ACI is read by its one path in the stored frame, and one that will not read is a problem on the recipient (`aci`, `CoercionFailed`), not a silent loss |
| Messages | the number or Apple ID address | each tapback's |
| Google Chat and Voice, SMS backup | the address or number | |
| address books, LinkedIn, Facebook | a card's numbers and addresses, in the account | |
| Beeper | none yet: a Matrix user id has no kind | |

The account itself, "Me", never has a handle.

## In a document

chat-common writes an author with a handle as a chip link in the
message header, and recipients as a line straight under it; the shape,
and why the href is load-bearing, is
[`chat-common/README.md`](../../datalib/backend/etl/chat-common/README.md)
§"The message header". A reactor with a handle is a chip link too, in a
message's reaction list and in the list of reactions to messages not
in the mirror (`render.rs::reactor`). Slack writes a `<@U…>` mention as
a chip link in the body (`slack_render/src/render/mrkdwn.rs`), and as
plain `@Name` inside code, which shows what it holds. Every chip link
is shaped by one function, `datalib_etl_render::message::chip_link`.

A chip anywhere in a body is safe for one reason: **it shows who the
href resolves to, never the link text**. A sender who writes
`[Picard](mailto:phisher@x)` gets a chip for the phisher under the
name the sources know, with "shown here as Picard" on hover.

## Accounts: `DatalibContact`

[`datalib/backend/contact_schema`](../../datalib/backend/contact_schema/src/lib.rs)
is only the shape: `source_id` and `key` say who describes the person;
then `kind` (`person` or `group`), `names` (the preferred one first),
`handles` (each a `ContactHandle`: the value as written, its `Handle`
if one could be made, a label, and `stopped_working_by`), `photo`
(the image, or a URL only a fetch could follow), `photo_url` (where
the app serves it; see below), `org`, `title`, `note`, `details`,
`groups`, `members`, `source_url`, the record's own stamps, and `seen`
(how much of a chat the person wrote).

Four producers, each needing less of a provider than the one before:

- **The contacts app**, from its store, with `source_id =
  "datalib_contacts"`.
- **A source about people**, through `contact-common`
  ([`etl/contact-common`](../../datalib/backend/etl/contact-common/src/render.rs)):
  an address-book card, a LinkedIn connection, a Facebook friend. Each
  is a document of its own, and carries its account.
- **A chat provider, for what only it knows**, in
  `NormalizedChat::contacts`: Slack's profiles (names, title, avatar,
  the email), WhatsApp's address book (`JidNames::contact`), Signal's
  recipients (number and ACI together). These are rows, not documents.
- **chat-common, for free** (`chat-common/src/people.rs`): a baseline
  per handle per document from what the provider showed: each author
  with the names it wrote under, how many items and the last one's
  stamp; each recipient and each reactor under the name shown, having
  written nothing. A provider's own account for the same handle
  replaces the baseline and keeps its count (`document_contacts`).

Every `RenderedMarkdown` carries its `contacts`, and the render store
and the index hold them in two tables: `source_contacts`
(`markdown_uuid`, `contact_key`, `source_id`, `name`, `seen_items`,
`last_seen_at`, and the whole account as `contact_json`) and
`source_contact_handles` (`markdown_uuid`, `contact_key`, `handle`,
indexed by handle). `grid_index` loads them the way it loads
`grid_rows`.

**`POST /people` on the `unified_index` applet** takes `{"handles":
[…]}` and answers `{"people": {<handle>: [DatalibContact, …]}}`: for
each handle, every account holding it, one per source (the rows from
each document summed), ranked by `unified_index/src/people.rs`: a
source about people first, then the chat where the person wrote the
most. A handle no source mentions is absent. This works with no
contacts app at all.

### Photos

`photo` is what the source gave: the image itself (`Photo::Inline`,
written beside the rendered page as `blobs/<uuid>.<ext>` and never
stored as a row) or a URL nothing fetched (`Photo::Url`, a Slack
avatar). `photo_url` is what a chip draws: a path on the app's own
origin, never another host's, since the app fetches nothing remote
unasked, and only for an image a browser draws: png, jpeg, gif or
webp (`contact_schema::is_drawable_photo`, the one list, which the
contacts app's photo rule shares). contact-common fills it with
`/applet/unified_index/asset/<markdown_uuid>/blobs/<file>`, which the
index's asset route serves; a photo of another type keeps its file
beside the page and gets no URL. The contacts app fills it with
`/applet/datalib_contacts/photo/<contact_id>` for a photo a person put
on their contact. `None` draws the person's initial, and so does an
image the browser fails to load.

## The contacts app

`<data_root>/datalib_curated/datalib_contacts/contacts.doltlite_db`,
written only by the `datalib_contacts` applet
([`datalib/backend/contacts`](../../datalib/backend/contacts/src/lib.rs)
is the store, `applets/src/datalib_contacts.rs` the routes). It is a
person's work, so the rules that make it irreplaceable:

- **Every write is a commit** on `datalib_writer`, sealed onto `main`
  (`etl/README.md` §"Connection pools"), so the history is an audit
  trail.
- **Nothing resets it.** A reset or removal of a source or the index
  never touches `datalib_curated/`. Every shape change is a rung on
  `LADDER`, never a rebuild (`doltlite_raw::open_curated` refuses a
  shape it cannot reach additively).
- **It is optional.** Take its `[[applets]]` entry out of the config
  and every source, the index and search keep working; chips draw what
  the sources know and offer nothing to link (the gateway's 502
  "no applet" is what `contacts.ts::isAbsent` reads).

| table | key | holds |
|---|---|---|
| `contacts` | `contact_id`, a random v4 (the one id here that is a person's act, not a function of a record) | `kind`, `name`, `note`, `merged_into`, stamps |
| `handles` | `handle`, as `datalib_handle` spells it | `contact_id`, `linked_how`, `linked_at_utc`, `stopped_working_by` |
| `members` | `(group_id, member_id)` | `added_at_utc` |
| `photos` | `contact_id` | `content_type`, `bytes`, `set_at_utc` |

Two rules the tables encode: **a handle belongs to exactly one
contact** (linking one someone else holds is refused, never taken
over; a shared address belongs to a group contact), and **keys are
handles, never `grid_rows.uuid`**, which moves when a recipe changes.
`stopped_working_by` is a date *by* which the handle had stopped
working: an old number still names its owner in every message from
when it worked, so the link stays and the chip marks it as old. "By",
not "on": marking a handle fills in today, which is always true when
nothing better is known, and the person narrows it if they remember. It
is a partial date (`2019`, `2019-06`, `2019-06-14`), a date a person
remembers rather than an instant anything measured, so it is not an
`_at_utc` stamp. A number *reassigned* to someone else is not covered:
the key is the handle alone, so a handle has one owner for all time.

The routes, each behind the gateway's secret
([`applets.md`](applets.md)), answering a refusal the store explains
(a handle someone else holds, a bad date, not a photo) as a 409 with
the store's words:

| route | does |
|---|---|
| `POST /resolve` | `{handles}` → each handle's contact as a `DatalibContact`, by handle; a handle nobody holds is absent |
| `GET /search?q=` | contacts whose name contains `q`, for the popover's typeahead |
| `POST /contacts` | create, with `name`, optional `kind`, and the `handles` to link at once |
| `GET /contact/{id}` | one contact, working handles first |
| `POST /link`, `POST /unlink` | one handle to or from a contact |
| `POST /stopped_working` | `{handle, by}`; `by: null` means it works again |
| `POST /rename` | |
| `GET`, `PUT`, `DELETE /photo/{id}` | the photo as bytes; put one (the body, with its `Content-Type`: png, jpeg, gif or webp, at most 4 MB; the route reads up to the gateway's 8 MB so the store's rule is the one that answers); drop it |

The config entry is `[[applets]] id = "datalib_contacts"` with
`command = "datalib-applet datalib_contacts"`; the gateway passes the
data root in the environment.

## In the UI

Everything is in `datalib/ui/src/cards/`:

- `chipLinks.js` is the markdown-it plugin: an explicit link whose
  href `handleFromUri` reads becomes `<a class="chip" data-handle=…>`;
  a link `linkify` made from a bare address in running text is left
  alone, so a signature's address stays an address. Plain JavaScript,
  so the render preview runs it too. It mirrors `to_uri` and
  `from_uri` over the same test cases.
- `contacts.ts` holds the pure rules, unit-tested in `contacts.test.ts`:
  `chipLook` (what a chip shows, from the contact if there is one,
  else the best-ranked account, else what the source showed),
  `chipTooltip` (the chip's title: who, the identifier, each source's
  account and the person's other handles), `chipMenu` (copy the name, the identifier or both; find
  everything from the person; link or edit), `copyText` and the copy
  rewrite. `people` is who each handle is, for the whole app: an
  instance of `resolver.ts`, the one resolver every document and grid
  asks. A chip asks as it is drawn, one drawing pass is one request to
  `/people` and the contacts app's `/resolve`, answers are kept, and
  an edit (create, link, unlink, no longer works) forgets the handles
  it touched, so every open document and grid draws them again.
  `decorateHandles` is the one function that touches a document's DOM:
  it collects the chips under a body, asks `people`, and draws.
  `chipCell` draws the same chip in a grid cell.
- `ChatBody.ce.vue` runs the decorate pass over a document's frame and
  owns the popover
  (`HandlePopover.ce.vue`: link to a contact, create one, unlink, mark
  a handle as no longer working) and the right-click menu
  (`ChipMenu.ce.vue`). `chip.css` is the one look.
- The grid's Author column is a chip too: `grid_rows.author_handle`
  (the `author_handle:` filter) comes with each message's row and each
  reaction's, and
  `GridCard.ce.vue` draws each Author cell from `people` and redraws
  them when an answer changes. The applet names the mark for a handle's
  kind in `columns.rs::handle_mark`.

A chip ranks what it hears: your contact first, then the accounts as
`/people` ranked them, then the text the source showed.

## What re-renders when

| change | what moves |
|---|---|
| the handle rules (`RULES_VERSION`) | every source re-renders; the contacts store takes a ladder rung |
| a new handle kind | nothing stored; the six places above |
| a provider's handles or accounts | that provider's `RENDER_VERSION` |
| the header, the recipients line, or what `people.rs` counts | chat-common's `LAYOUT_VERSION`, which re-renders every chat source |
| what contact-common writes | the `RENDER_VERSION` of each source that uses it (contacts, linkedin, facebook) |
| the contacts store's shape | a rung on `LADDER`; never a reset |

## Where it is tested

- `handle`: every constructor's spellings, `parse` against `rebuild`,
  the URI round trip over every kind.
- `contacts`: the store end to end on a real doltlite file, the ladder
  against the current rules, the photo rules.
- `chat-common`: `people.rs` (the baseline, recipients, reactors, the
  provider merge); `render.rs` (the header and recipients line).
- `unified_index`: `people.rs` ranking; the applet's `/people` against
  the fixture index.
- `tests/fixtures/ingested_tng_test.py`: the end-to-end guard, from the
  TNG fixture through the index: Picard known by one address from
  three sources, one number spelled three ways as one handle, WhatsApp's
  address book, Signal's number and ACI together, a Slack mention as a
  chip link, two cards' photos as the URLs the index serves.
- UI: `contacts.test.ts`, `resolver.test.ts` and
  `tests/chip_links.test.ts`; the render preview golden shows
  unresolved chips. `tests/e2e/contacts.spec.ts` runs on a root with the
  contacts app: it links Riker's Slack and email handles to one contact
  from two documents and checks the documents and an open grid follow.

## Not built

The contact card, merge, groups and members, undo, the triage grid of
unresolved handles, `row_handles`, the `contact:` search filter,
mentions outside Slack, and a handle for a Beeper (Matrix) user:
[`plans/contacts.md`](plans/contacts.md) §"Order of work" and
[`plans/chips.md`](plans/chips.md) §"Order of work".
