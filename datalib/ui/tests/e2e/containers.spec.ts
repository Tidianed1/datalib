import { test, expect, type Page } from "@playwright/test";

// The containers layout: tabs down the side, each holding cards or
// containers. The Dashboard is a composite of four cards with "solidify
// all" on, so it looks like one page and a card opened from it gets a
// tab of its own; with that off, the card lands inside it instead. The
// tree is kept in the library, so these share one saved layout and run
// in order, each starting from a cleared one.

test.describe.configure({ mode: "serial" });

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem("datalib-layout", "containers");
    localStorage.setItem("datalib-dev-mode", "0");
  });
  const cleared = await page.request.put("/api/ui/state/layout", {
    data: "null",
    headers: { "content-type": "application/json" },
  });
  expect(cleared.status()).toBe(204);
});

const tabs = (page: Page) => page.locator(".ct-tab");
const mainCards = (page: Page) => page.locator(".ct-main .ct-card");

async function savedTabCount(page: Page): Promise<number> {
  const r = await page.request.get("/api/ui/state/layout");
  if (!r.ok()) return -1;
  const tree = (await r.json()) as { children?: unknown[] } | null;
  return tree?.children?.length ?? -1;
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

test("in dev mode, turning off solidify all opens the card inside the Dashboard", async ({
  page,
}) => {
  await page.goto("/");
  await expect(mainCards(page)).toHaveCount(4);
  await page.getByRole("button", { name: "Dev", exact: true }).click();
  // Dev mode shows the solidified container's controls.
  const solidifyAll = page.locator(".ct-box-head").getByLabel("Solidify all");
  await expect(solidifyAll).toBeChecked();
  await solidifyAll.uncheck();

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
