import { test, expect, type APIRequestContext, type Page } from "@playwright/test";
import {
  EVERY_ROW,
  selectRowByUuid,
  gridSettled,
  inDocFrame,
  searchAndSettle,
} from "./grid-helpers";

// The contacts app end to end, on a root that has it (`contactsRoot` in
// playwright.config.ts). Riker appears under two handles from two
// sources in the TNG fixture: a Slack user in the Slack source, an email
// address in the Google Takeout chats. A person makes a contact from one
// chip in a document, links the other handle to it from a chip in
// another source's document, and from then on both chips, in documents
// and in the grid's Author column, show the contact rather than what
// either source called him (docs/dev/plans/chips.md).

const SLACK = "slack:T_NCC1701D/U_RIKER";
const EMAIL = "email:riker@enterprise.starfleet";
// Neither source's name for him, so a chip showing it was resolved
// through the contact and not drawn from the source.
const CONTACT = "Number One";

type Row = {
  uuid: string;
  conversation_uuid: string;
  markdown_uuid: string | null;
  message_index: number | null;
};

const byHandle = (handle: string) => `author_handle:"${handle}"`;

/// A message Riker wrote under `handle`, its document opened from the
/// grid; the conversation it is in.
async function openMessageBy(
  page: Page,
  request: APIRequestContext,
  handle: string,
): Promise<string> {
  const q = byHandle(handle);
  const resp = await request.get(
    `/applet/unified_index/search?q=${encodeURIComponent(q)}&limit=50`,
  );
  expect(resp.ok()).toBeTruthy();
  const { rows } = (await resp.json()) as { rows: Row[] };
  const message = rows.find((r) => r.markdown_uuid && r.message_index != null);
  expect(message, `the fixture must have a message by ${handle}`).toBeTruthy();
  await page.goto(EVERY_ROW);
  await searchAndSettle(page, q);
  await gridSettled(page);
  await selectRowByUuid(page, message!.uuid);
  return message!.conversation_uuid;
}

/// The chip for `handle` in the open document, once the document has
/// asked the contacts app about it: linkable when no contact holds it.
async function chipIn(page: Page, handle: string) {
  return (await inDocFrame(page, `a.chip[data-handle="${handle}"]`)).first();
}

test("two handles from two sources linked to one contact show it in documents and the grid", async ({
  page,
  request,
}) => {
  test.setTimeout(120_000);
  const popover = page.locator(".handle-popover");

  // 1. Riker's Slack message: the chip offers a link, and a new contact
  //    takes the Slack handle.
  const slackConversation = await openMessageBy(page, request, SLACK);
  const slackChip = await chipIn(page, SLACK);
  await expect(slackChip).toHaveClass(/handle-linkable/, { timeout: 15_000 });
  await slackChip.click();
  await expect(popover).toBeVisible();
  await popover.getByLabel("Contact name").fill(CONTACT);
  await popover.getByRole("button", { name: `New contact “${CONTACT}”` }).click();
  await expect(popover).toBeHidden();
  await expect(slackChip).toHaveClass(/handle-resolved/);
  await expect(slackChip).toHaveText(new RegExp(`${CONTACT}$`));

  // 2. Riker's Google Chat message, in another source: the email handle
  //    is not anyone's yet, and the popover finds the contact to link it to.
  const emailConversation = await openMessageBy(page, request, EMAIL);
  const emailChip = await chipIn(page, EMAIL);
  await expect(emailChip).toHaveClass(/handle-linkable/, { timeout: 15_000 });
  await emailChip.click();
  await expect(popover).toBeVisible();
  await popover.getByLabel("Contact name").fill("Number");
  await popover.getByRole("button", { name: `Link to ${CONTACT}` }).click();
  await expect(popover).toBeHidden();
  await expect(emailChip).toHaveClass(/handle-resolved/);
  await expect(emailChip).toHaveText(new RegExp(`${CONTACT}$`));

  // 3. The store says both handles are the one contact.
  const resolved = await request.post("/applet/datalib_contacts/resolve", {
    data: { handles: [SLACK, EMAIL] },
  });
  expect(resolved.ok()).toBeTruthy();
  const who = (
    (await resolved.json()) as { resolved: Record<string, { key: string; names: string[] }> }
  ).resolved;
  expect(who[SLACK]?.names[0]).toBe(CONTACT);
  expect(who[EMAIL]?.key, "both handles belong to the same contact").toBe(who[SLACK]?.key);

  // 4. The grid's Author column shows the contact for each handle's rows,
  //    resolved by the grid itself, not by the document view. Each grid is
  //    the message's conversation rather than the one author's rows: the
  //    grid hides a column whose values are all the same, so a grid of one
  //    author has no Author column to look at.
  for (const [handle, conversation] of [
    [SLACK, slackConversation],
    [EMAIL, emailConversation],
  ] as const) {
    await page.goto(EVERY_ROW);
    await searchAndSettle(page, `convo:${conversation}`);
    await gridSettled(page);
    const cell = page.locator(`.grid-box .slick-cell a.chip[data-handle="${handle}"]`).first();
    await expect(cell).toHaveClass(/handle-resolved/, { timeout: 15_000 });
    await expect(cell).toHaveText(new RegExp(`${CONTACT}$`));
  }
});
