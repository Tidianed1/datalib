# Testing the wizard's latchkey sign-in flows

**Status: the hermetic tier is built (2026-10-05); the keychain tier,
the real-machine tier and the product fixes below are proposals.** How
the built part works is in
[`datalib/ui/tests/e2e_auth/README.md`](../../../datalib/ui/tests/e2e_auth/README.md);
this page is what it found and what is left.

## 0. Why

Signing in is one of the first things a person does with datalib, and
until this suite nothing ran it: every wizard spec stubbed
`/api/latchkey/*`. Four complaints had nothing to pin them:

- the flow differs from source to source;
- the scenarios a person meets go untested: latchkey set up on the
  command line beforehand, a pasted token, a gateway under Minds;
- latchkey runs before the person has asked for anything, and on a mac
  that means a keychain prompt;
- the error in the dialog is sometimes a wall of text.

## 1. Facts the suite rests on

Read from latchkey 3.16's source and measured on a mac the same day.

- **Every latchkey run reads the keychain**, `--version` and
  `services info` included: `cli.js` resolves its encryption key before
  it parses arguments. Only `LATCHKEY_ENCRYPTION_KEY` or
  `LATCHKEY_GATEWAY` skips it. `LATCHKEY_KEYRING_SERVICE_NAME` only
  renames the item.
- **The wizard runs latchkey when a tile is picked**:
  `SourceWizard.vue` watches `service` with `immediate: true` and calls
  `GET /api/latchkey/<svc>`, which runs `services info` — without
  `--offline`, so a stored credential is also checked over the network.
  With one stored, that took 6.4s in the suite.
- **latchkey hard-codes a visible browser window** for `auth browser`;
  the only lever is the executable `ensure-browser` records.
- **latchkey's own credential check goes out as a plain curl**: it runs
  through `LATCHKEY_CURL` without datalib's impersonation marker, so for
  a host behind Cloudflare's bot wall it would be refused whatever the
  credential.
- **A `latchkey gateway` binds `localhost`**, which did not answer on
  127.0.0.1 here; the suite passes `--host 127.0.0.1`.

## 2. What the suite found

Each is a spec marked `test.fail()` until the product is fixed.

| Finding | Spec |
|---|---|
| Picking a tile runs `services info` — a keychain read on a mac — before any click. | `first-contact.spec.ts` |
| With no runtime, Slack's Connection section offers no way to sign in and says nothing: the "Couldn't ask latchkey" note lives inside the account picker, and Slack has none. | `first-contact.spec.ts` |
| With no browser, the login fails with a 284-character paragraph telling the person to run `npx -y latchkey@3.16.2 ensure-browser` — a command, on a screen whose job is to run it, naming `npx` in an app that ships latchkey. | `browser-login.spec.ts` |

## 3. What is left

**Product fixes**, each against its failing spec:

1. Don't run latchkey on tile pick. The account list is the hard part:
   account names live inside the encrypted store, so listing them needs
   the key. Options: list them on a click ("Show stored accounts"); show
   the accounts datalib's own config already names, which needs no
   latchkey; or ask latchkey upstream for an unencrypted index of
   service and account names, or for a `services info` that does not
   resolve the key.
2. Say why the Connection section is empty, for every source, not only
   those with an account picker.
3. When no browser is found, offer a button that fetches one
   (`ensure-browser --source download-playwright-browser`, a few hundred
   MB, said on the button), instead of a command.
4. Errors in one line. `connect` returns the last 4 KiB of stderr
   unscrubbed; `probe` returns the whole stderr of `datalib-step`, whose
   first line can be a JSON log record.

**Coverage still missing:**

- **The keychain tier (macOS).** Leave `LATCHKEY_ENCRYPTION_KEY` unset
  and give each run its own `LATCHKEY_KEYRING_SERVICE_NAME`, deleted at
  teardown; on a GitHub runner, also a throwaway default keychain
  (`security create-keychain`, `default-keychain -s`, `unlock-keychain`,
  `set-keychain-settings`). Never the second half on a laptop: it
  changes the person's keychain search list. Scenarios: first run
  creates the key; `.enc` present but the item gone ("encryption key was
  lost"); a locked keychain (latchkey's 30s timeout); an item written by
  one `node` and read by another — the command-line Node versus the
  bundled one, which is the likeliest cause of the prompts people see.
  Measure that last one before designing around it.
- **The real-machine tier (CI VM only).** Point `/etc/hosts` at the
  fake, trust a throwaway CA, and drop the curl shim and the browser
  wrapper: a headed browser, the real network stack, nothing between
  latchkey and the "site" but DNS. The specs stay the same; the harness
  switches on one variable.
- **Fastmail's and Google's OAuth logins**, which need a fake
  authorization server.
- **Uniformity**: one table-driven spec over every credentialed catalog
  entry, where a source that is meant to differ says so in the table.
- **Scenarios not yet written**: two stored accounts and the "No
  credentials stored for account" retry; a gateway that is down; a login
  page that never hands out a credential (needs a shorter connect
  timeout for tests).
- **Linux.** The suite sets `DISPLAY` for latchkey's check there, but
  has only run on a mac.
