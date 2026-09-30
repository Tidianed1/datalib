import { test, expect } from "@playwright/test";
import { stubClipboard } from "./grid-helpers";

// The chrome around the cards: the toolbar's search box opens a search
// card on what was typed, and ⌘K (Ctrl+K) reaches it from anywhere;
// the status bar's "Logs" reveals the log once. All act on the
// URL-synced miller stack, so the path says what they did. A new
// window opens on Home.

async function stackPath(page: import("@playwright/test").Page): Promise<string> {
  return decodeURIComponent(await page.evaluate(() => location.pathname));
}

const searchBox = (page: import("@playwright/test").Page) =>
  page.getByRole("searchbox", { name: "Search your data" });

test.describe("toolbar", () => {
  test("a new window opens on Home", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator(".miller-col")).toHaveCount(1);
    await expect(page.locator(".miller-col-title")).toHaveText("Home");
  });

  test("the search box opens a search card on what was typed", async ({ page }) => {
    await page.goto("/");
    // The card surface is up once Home is: a search asked before that
    // replaces the stack instead of opening beside it.
    await expect(page.locator(".miller-col-title")).toHaveText("Home");
    await searchBox(page).fill("warp");
    await searchBox(page).press("Enter");
    await expect(page.locator(".miller-col")).toHaveCount(2);
    await expect(page.locator(".miller-col-title").last()).toHaveText("Search: warp");
    expect(await stackPath(page)).toContain('searchView({"q":"warp"})');
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
    const col = page.locator(".miller-col").filter({ has: page.locator(".rl-panel") });
    await expect(col).toBeVisible({ timeout: 10_000 });
    await expect(col.locator(".miller-col-title")).toHaveText("Log · everything");
    expect(await stackPath(page)).toContain("logView()");

    await page.getByRole("button", { name: "Logs" }).click();
    await expect(page.locator(".miller-col")).toHaveCount(2);
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

  test("the syncing pill is absent when nothing runs", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator(".datalib-toolbar")).toBeVisible();
    await expect(page.locator(".sync-indicator")).toHaveCount(0);
  });
});
