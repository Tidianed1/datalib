import { test, expect, type Page } from "@playwright/test";

// The containers layout: tabs down the side, each holding cards or
// containers. The Dashboard is a solidified composite of four cards, so
// it looks like one page and a card opened from it gets a tab of its
// own; unsolidified, the card lands inside it instead. The
// tree is kept in the library, so these share one saved layout and run
// in order, each starting from a cleared one.

test.describe.configure({ mode: "serial" });

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.removeItem("datalib-layout-unsaved");
    localStorage.setItem("datalib-edit-mode", "0");
  });
  const cleared = await page.request.put("/api/ui/state/layout", {
    data: "null",
    headers: { "content-type": "application/json" },
  });
  expect(cleared.status()).toBe(204);
});

const tabs = (page: Page) => page.locator(".ct-tab");
const mainCards = (page: Page) => page.locator(".ct-main .ct-card");

type SavedTab = { name?: string | null };

async function savedTabs(page: Page): Promise<SavedTab[] | null> {
  const r = await page.request.get("/api/ui/state/layout");
  if (!r.ok()) return null;
  const tree = (await r.json()) as { children?: SavedTab[] } | null;
  return tree?.children ?? null;
}

async function savedTabCount(page: Page): Promise<number> {
  return (await savedTabs(page))?.length ?? -1;
}

test("the Dashboard is four cards that read as one page", async ({ page }) => {
  await page.goto("/");
  await expect(tabs(page)).toHaveCount(1);
  await expect(tabs(page)).toContainText("Dashboard");
  await expect(mainCards(page)).toHaveCount(4);
  await expect(page.locator(".ct-card-head, .ct-box-head")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Open Sources" })).toBeVisible();
});

test("a card opened from the Dashboard gets a tab of its own", async ({ page }) => {
  await page.goto("/");
  await expect(mainCards(page)).toHaveCount(4);
  await page.getByRole("button", { name: "Open Sources" }).click();
  await expect(tabs(page)).toHaveCount(2);
  await expect(tabs(page).nth(1)).toHaveClass(/is-selected/);
  await expect(mainCards(page)).toHaveCount(1);

  // The Dashboard kept its shape.
  await tabs(page).first().click();
  await expect(mainCards(page)).toHaveCount(4);
});

test("in edit mode, unsolidifying the Dashboard opens the card inside it", async ({ page }) => {
  await page.goto("/");
  await expect(mainCards(page)).toHaveCount(4);
  await expect(page.locator(".ct-foldertab")).toHaveCount(0);
  await page.getByRole("button", { name: "Edit", exact: true }).click();
  // Edit mode shows the solidified container, and its menu turns that off.
  await page.locator(".ct-foldertab").click();
  const solidified = page.getByRole("menuitemcheckbox", { name: "Solidified" });
  await expect(solidified).toHaveAttribute("aria-checked", "true");
  await solidified.click();

  await page.getByRole("button", { name: "Open Sources" }).click();
  await expect(mainCards(page)).toHaveCount(5);
  await expect(tabs(page)).toHaveCount(1);
});

test("the layout is kept in the library across a reload", async ({ page }) => {
  await page.goto("/");
  await expect(mainCards(page)).toHaveCount(4);
  await page.getByRole("button", { name: "Open Sources" }).click();
  await expect(tabs(page)).toHaveCount(2);
  await expect.poll(() => savedTabCount(page), { timeout: 10_000 }).toBe(2);

  await page.reload();
  await expect(tabs(page)).toHaveCount(2);
  await expect(tabs(page).nth(1)).toHaveClass(/is-selected/);
});

test("a tab the person renames keeps its name after a reload", async ({ page }) => {
  await page.goto("/");
  await expect(mainCards(page)).toHaveCount(4);
  await page.getByRole("button", { name: "Open Sources" }).click();
  await expect(tabs(page)).toHaveCount(2);
  await tabs(page).nth(1).getByTitle("more").click();
  await page.getByRole("menuitem", { name: "Rename…" }).click();
  await page.getByLabel("Name").fill("My sources");
  await page.getByRole("button", { name: "OK" }).click();
  await expect(tabs(page).nth(1)).toContainText("My sources");

  await expect
    .poll(async () => (await savedTabs(page))?.[1]?.name, { timeout: 10_000 })
    .toBe("My sources");
  await page.reload();
  // The card names itself again as it mounts; the person's name stays.
  await expect(mainCards(page)).toHaveCount(1);
  await expect(tabs(page).nth(1)).toContainText("My sources");
});

test("a composite cannot take a built-in composite's name", async ({ page }) => {
  await page.goto("/");
  await expect(mainCards(page)).toHaveCount(4);
  await tabs(page).first().getByTitle("more").click();
  await page.getByRole("menuitem", { name: "Save as composite…" }).click();
  await page.getByLabel("Save as composite").fill("Dashboard");
  await page.getByRole("button", { name: "OK" }).click();
  await expect(page.getByText('"Dashboard" is a built-in composite')).toBeVisible();
  await page.getByRole("button", { name: "Cancel" }).click();
  await expect(page.getByLabel("Save as composite")).toHaveCount(0);
});
