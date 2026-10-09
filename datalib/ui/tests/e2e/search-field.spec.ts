// The search field: typing a key offers it, taking it offers its values
// from the table, and a source picked is drawn as its chip in the text,
// over the query it still is (docs/dev/plans/search_autocomplete.md).

import { test, expect, type Page } from "@playwright/test";
import { GRID, shownCards } from "./grid-helpers";

const field = (page: Page) => shownCards(page).getByTestId("search-input");
const menu = (page: Page) => shownCards(page).locator(".cm-tooltip-autocomplete");

test("a key, then a source picked from its values, is drawn as the source's chip", async ({
  page,
}) => {
  await page.goto(GRID);
  await field(page).fill("is:document");
  await field(page).press("End");
  await field(page).pressSequentially(" sou");
  await expect(menu(page).getByText("source_id:", { exact: true })).toBeVisible();
  // Tab takes the first suggestion, and taking a key asks for its values.
  await field(page).press("Tab");
  await expect(field(page)).toHaveAttribute("data-query", "is:document source_id:");
  const slack = menu(page).locator('a.chip[data-entity="datalib:group/slack"]');
  await expect(slack).toBeVisible();

  // Most documents first: slack's 12 ahead of slack-diff's 3.
  await field(page).pressSequentially("slac");
  await expect(menu(page).locator("li").first().locator("a.chip")).toHaveAttribute(
    "data-entity",
    "datalib:group/slack",
  );
  await field(page).press("Tab");
  await expect(field(page)).toHaveAttribute("data-query", "is:document source_id:slack ");
  await expect(field(page).locator('a.chip[data-entity="datalib:group/slack"]')).toBeVisible();

  // Backspace at the chip's end takes it whole; the key stays.
  await field(page).press("End");
  await field(page).press("Backspace");
  await field(page).press("Backspace");
  await expect(field(page)).toHaveAttribute("data-query", "is:document source_id:");
});

/// Enter with nothing chosen searches what was typed: a value taken by
/// hand is a search, not a pick.
test("Enter with no suggestion chosen leaves the typed value as it is", async ({ page }) => {
  await page.goto(GRID);
  await field(page).fill("");
  await field(page).pressSequentially("source_id:sla");
  await expect(menu(page)).toBeVisible();
  await field(page).press("Enter");
  await expect(field(page)).toHaveAttribute("data-query", "source_id:sla");
});
