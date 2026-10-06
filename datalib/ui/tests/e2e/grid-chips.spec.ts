import { test, expect } from "@playwright/test";
import { SEARCH_MENU, cardOf, searchMenuItem, stubClipboard } from "./grid-helpers";

// The search grid's Author cell is the same chip a document draws
// (docs/dev/plans/chips.md § "In a grid"): a link with the handle, the
// name the source showed, the kind's mark. Right-click on it offers the
// chip's entries ahead of the row's; a double-click narrows the grid to
// everything from that person. The fixture has no contacts app, so the
// chips are unresolved; the shape and the clicks are what every root has.

type Row = { uuid: string; author_handle: string | null; author_ref: { id: string } | null };

// Slack's authors all carry a handle, and the grid draws only the rows in
// view, so the grid is opened on Slack rather than on everything.
const SLACK_ROWS = "/gridView()::q%3Dsource_id%3Aslack";

async function anAuthorChip(page: import("@playwright/test").Page) {
  await page.goto(SLACK_ROWS);
  await page.locator(".grid-box .slick-row").first().waitFor({ timeout: 10_000 });
  const chip = page.locator(".grid-box .slick-cell a.chip[data-handle]").first();
  await expect(chip).toBeVisible({ timeout: 10_000 });
  return chip;
}

test("an author with a handle is drawn as a chip link", async ({ page, request }) => {
  const resp = await request.get("/applet/unified_index/search?q=source_id%3Aslack&limit=200");
  expect(resp.ok()).toBeTruthy();
  const { rows } = (await resp.json()) as { rows: Row[] };
  const withHandle = rows.find((r) => r.author_handle);
  expect(withHandle, "the fixture must have a row whose author has a handle").toBeTruthy();
  // The applet sends the handle as the identity's id, as a URI.
  expect(withHandle!.author_ref?.id).toMatch(/^(mailto:|tel:|slack:)/);

  const chip = await anAuthorChip(page);
  await expect(chip).toHaveAttribute("href", /^(mailto:|tel:|slack:)/);
  await expect(chip).toHaveClass(/handle-unresolved/);
  await expect(chip).not.toHaveText("");
});

test("right-click on the chip offers its copies, and a double-click narrows to the person", async ({
  page,
}) => {
  const chip = await anAuthorChip(page);
  const handle = (await chip.getAttribute("data-handle"))!;
  const value = handle.slice(handle.indexOf(":") + 1);
  await stubClipboard(page);

  await chip.click({ button: "right" });
  await expect(page.locator(SEARCH_MENU)).toBeVisible({ timeout: 5_000 });
  await searchMenuItem(page, `Copy ${value}`).click();
  await expect
    .poll(() => page.evaluate(() => (window as unknown as { __copied?: string }).__copied))
    .toBe(value);

  await chip.dblclick();
  // The query bar carries the term; the value is bare or quoted as the
  // grammar needs, so match on the key and the handle's text.
  const bar = page.locator('[data-testid="search-input"]');
  await expect(bar).toHaveValue(/author_handle:/);
  await expect(bar).toHaveValue(new RegExp(value.replaceAll("+", "\\+")));
});

/// The Source cell is a group chip (docs/dev/plans/chips.md): datalib-http
/// answers what the group is now — its name, its type, its status — the
/// menu copies its id, and a double-click opens its sync dashboard.
test("a row's Source is a group chip that resolves, copies its id and opens its dashboard", async ({
  page,
}) => {
  await page.goto(SLACK_ROWS);
  await page.locator(".grid-box .slick-row").first().waitFor({ timeout: 10_000 });
  const chip = page
    .locator('.grid-box .slick-cell a.chip[data-entity="datalib:group/slack"]')
    .first();
  await expect(chip).toBeVisible({ timeout: 10_000 });
  // Resolved: the hover goes past the name to what the group is and its
  // status, which only `/api/entities` knows.
  await expect(chip).toHaveAttribute("title", /\(slack\)\n.+\n.+/, { timeout: 10_000 });

  await stubClipboard(page);
  await chip.click({ button: "right" });
  await expect(page.locator(SEARCH_MENU)).toBeVisible({ timeout: 5_000 });
  await searchMenuItem(page, "Copy slack").click();
  await expect
    .poll(() => page.evaluate(() => (window as unknown as { __copied?: string }).__copied))
    .toBe("slack");

  await chip.dblclick();
  await expect(cardOf(page, 'syncDashboardView({"group":"slack"})')).toBeVisible({
    timeout: 10_000,
  });
});
