// The Search card: one query, shown as a list with a preview or as a
// table, with source chips and "Meaning only" that write the query the
// person could have typed.

import { test, expect, type Page } from "@playwright/test";
import { GRID, searchGrid } from "./grid-helpers";

const input = (page: Page) => page.getByTestId("search-input");
const viewButton = (page: Page, name: string) => page.getByRole("button", { name, exact: true });
const results = (page: Page) => page.getByRole("list", { name: "Results" }).locator(".sc-result");
const listShows = (page: Page) => page.locator(".ct-main .sc-main");
const tableShows = (page: Page) => page.locator(".ct-main .grid-wrap");
const chips = (page: Page) => page.getByRole("group", { name: "Sources" }).getByRole("button");

test("the list and the table are two views of one query", async ({ page }) => {
  await page.goto("/searchView()");
  await expect(viewButton(page, "List and preview")).toHaveAttribute("aria-pressed", "true");
  await expect(results(page).first()).toBeVisible({ timeout: 10_000 });
  // The table asks nothing until it is shown.
  await expect(searchGrid(page)).toHaveCount(0);

  await input(page).fill("is:document -kind:nothing");
  await expect(listShows(page)).toHaveAttribute("data-shown-query", "is:document -kind:nothing");

  await viewButton(page, "Table").click();
  await expect(tableShows(page)).toHaveAttribute("data-shown-query", "is:document -kind:nothing");
  await expect(searchGrid(page)).toBeVisible();
  await expect(results(page).first()).toBeHidden();
  await expect(input(page)).toHaveValue("is:document -kind:nothing");

  // The view is kept with the card.
  await page.reload();
  await expect(viewButton(page, "Table")).toHaveAttribute("aria-pressed", "true");
  await expect(tableShows(page)).toHaveAttribute("data-shown-query", "is:document -kind:nothing");

  await viewButton(page, "List and preview").click();
  await expect(results(page).first()).toBeVisible({ timeout: 10_000 });
  await expect(searchGrid(page)).toBeHidden();
});

test("a source chip writes the source filter, and a typed one lights the chip", async ({
  page,
}) => {
  await page.goto(GRID);
  await expect(chips(page).first()).toContainText("All");
  await expect(chips(page).first()).toHaveAttribute("aria-pressed", "true");
  await chips(page).nth(1).click();
  await expect(input(page)).toHaveValue(/^is:document source_id:\S+$/);
  await expect(chips(page).nth(1)).toHaveAttribute("aria-pressed", "true");
  const narrowed = await input(page).inputValue();
  await expect(tableShows(page)).toHaveAttribute("data-shown-query", narrowed);

  await chips(page).first().click();
  await expect(input(page)).toHaveValue("is:document");
  await expect(chips(page).nth(1)).toHaveAttribute("aria-pressed", "false");

  await input(page).fill(narrowed);
  await expect(chips(page).nth(1)).toHaveAttribute("aria-pressed", "true");
});

test("Meaning only moves the typed words into qmd_vsearch, and back", async ({ page }) => {
  await page.goto(GRID);
  const meaning = page.getByRole("checkbox", { name: "Meaning only" });
  // Nothing to rank in a query of filters alone.
  await expect(meaning).toBeDisabled();
  await input(page).fill("is:document warp");
  await meaning.check();
  await expect(input(page)).toHaveValue("is:document qmd_vsearch:warp");
  await expect(meaning).toBeChecked();
  await meaning.uncheck();
  await expect(input(page)).toHaveValue("is:document warp");
});
