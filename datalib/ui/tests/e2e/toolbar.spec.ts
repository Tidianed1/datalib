import { test, expect } from "@playwright/test";
import { cardOf, cardTitle, shownCards, stubClipboard, tabLabels } from "./grid-helpers";

// The chrome around the cards: the toolbar's search box opens a search
// card on what was typed, and ⌘K (Ctrl+K) reaches it from anywhere;
// the status bar's "Logs" reveals the log once. Each opens a tab of
// its own. A new window opens on the Dashboard.

const searchBox = (page: import("@playwright/test").Page) =>
  page.getByRole("searchbox", { name: "Search your data" });

test.describe("toolbar", () => {
  test("a new window opens on the Dashboard", async ({ page }) => {
    await page.goto("/");
    await expect(tabLabels(page)).toHaveText(["Dashboard"]);
    await expect(shownCards(page)).toHaveCount(4);
  });

  test("the search box opens a search card on what was typed", async ({ page }) => {
    await page.goto("/");
    await expect(tabLabels(page)).toHaveText(["Dashboard"]);
    await searchBox(page).fill("warp");
    await searchBox(page).press("Enter");
    const card = cardOf(page, 'searchView({"q":"warp"})');
    await expect(card).toBeVisible();
    await expect(cardTitle(card)).toHaveText("Search: warp");
    await expect(tabLabels(page)).toHaveCount(2);
    // The box empties, ready for the next search.
    await expect(searchBox(page)).toHaveValue("");
  });

  test("Ctrl+K puts the caret in the search box", async ({ page }) => {
    await page.goto("/");
    await expect(searchBox(page)).not.toBeFocused();
    await page.keyboard.press("Control+k");
    await expect(searchBox(page)).toBeFocused();
  });

  test("the status bar's Logs opens the log over every run, once", async ({ page }) => {
    await page.goto("/");
    await page.getByRole("button", { name: "Logs" }).click();
    const col = shownCards(page).filter({ has: page.locator(".rl-panel") });
    await expect(col).toBeVisible({ timeout: 10_000 });
    await expect(cardTitle(col)).toHaveText("Log · everything");
    await expect(cardOf(page, "logView()")).toHaveCount(1);

    await page.getByRole("button", { name: "Logs" }).click();
    await expect(tabLabels(page)).toHaveCount(2);
    await expect(cardOf(page, "logView()")).toHaveCount(1);
  });

  test("the status bar's density switch resizes the chrome, and is kept", async ({ page }) => {
    await page.goto("/");
    const html = page.locator("html");
    await expect(html).toHaveAttribute("data-density", "compact");
    await page.getByRole("button", { name: "Comfortable" }).click();
    await expect(html).toHaveAttribute("data-density", "comfortable");
    await page.reload();
    await expect(html).toHaveAttribute("data-density", "comfortable");
  });

  test("the status bar copies the data root's path in a browser", async ({ page }) => {
    await page.goto("/");
    const path = page.getByTestId("root-storage").locator(".root-bar-path");
    await expect(path).not.toHaveText("", { timeout: 10_000 });
    const copied = await stubClipboard(page);
    await page.getByRole("button", { name: "Copy path" }).click();
    await expect(page.locator(".datalib-toast", { hasText: "path copied" })).toBeVisible();
    expect(await copied()).toBe(await path.textContent());
  });

  // The window's minimum width in the desktop shell (MIN_WINDOW_WIDTH)
  // assumes the search box shrinks first and to no less than 180px, and
  // that the library name then ellipsizes rather than sliding under it.
  test("the search box sits at the right end and shrinks with the window", async ({ page }) => {
    await page.goto("/");
    const box = page.locator(".command-box");
    const lib = page.locator(".crumb-lib");
    const name = page.locator(".crumb-name");
    await expect(box).toBeVisible();
    const wide = (await box.boundingBox())!;
    expect(wide.width).toBe(440);
    expect(1280 - (wide.x + wide.width)).toBeLessThan(16);
    const truncated = () => name.evaluate((el) => el.scrollWidth > el.clientWidth);

    for (const density of ["Compact", "Comfortable"]) {
      await page.setViewportSize({ width: 1280, height: 800 });
      await page.getByRole("button", { name: density }).click();
      for (const width of [600, 420]) {
        await page.setViewportSize({ width, height: 800 });
        const b = (await box.boundingBox())!;
        const l = (await lib.boundingBox())!;
        expect(b.width).toBeLessThan(440);
        expect(b.width).toBeGreaterThanOrEqual(180);
        expect(b.x + b.width).toBeLessThanOrEqual(width);
        expect(b.x).toBeGreaterThanOrEqual(l.x + l.width);
        // At 600px the search box has room to give; at 420px it is at
        // its floor and the name gives way.
        expect(await truncated()).toBe(width === 420);
      }
    }
  });

  test("the syncing pill is absent when nothing runs", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator(".datalib-toolbar")).toBeVisible();
    await expect(page.locator(".sync-indicator")).toHaveCount(0);
  });
});
