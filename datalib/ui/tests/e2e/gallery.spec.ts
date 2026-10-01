import { test, expect } from "@playwright/test";
import { GRID } from "./grid-helpers";

// Non-dev card creation goes through the new-card gallery: the "+"
// strip after the last miller column creates a `galleryView()` card —
// a list of parameter-less components — and picking an entry REPLACES
// that card via host.setSource. Components that need arguments hide
// behind a picker: the gallery's "Document" entry opens
// documentPickerView (a /applet/unified_index/docs listing), which in turn replaces
// itself with `documentView("<uuid>")` on pick.

test.describe("new-card gallery (non-dev mode)", () => {
  test("+ strip → gallery → Document → picker → document card", async ({ page }) => {
    await page.goto(GRID);
    // Non-dev: no source boxes, but the "+" creation strip is there.
    await expect(page.locator(".miller-col-source")).toHaveCount(0);
    await page.locator(".miller-add").click();

    // The gallery column appears, builtins listed with Home first.
    const galleryRows = page.locator(".gv-row");
    await expect(galleryRows.first()).toContainText("Home");
    expect(decodeURIComponent(await page.evaluate(() => location.pathname))).toContain(
      "galleryView()",
    );

    // Pick "Markdown Document" → the gallery card becomes the document picker.
    await galleryRows.filter({ hasText: "Markdown Document" }).first().click();
    const docRows = page.locator(".dp-row");
    await expect(docRows.first()).toBeVisible({ timeout: 10_000 });
    expect(decodeURIComponent(await page.evaluate(() => location.pathname))).toContain(
      "documentPickerView()",
    );

    // Pick the first document → the picker becomes that document.
    await docRows.first().click();
    await expect(page.locator(".chat-preview")).toBeVisible({ timeout: 10_000 });
    expect(decodeURIComponent(await page.evaluate(() => location.pathname))).toContain(
      'documentView("',
    );

    // Each pick was a navigation, so the browser's history walks them:
    // back to the picker, back to the gallery, forward to the picker.
    await page.goBack();
    await expect(docRows.first()).toBeVisible({ timeout: 10_000 });
    await page.goBack();
    await expect(page.locator(".gv-row").first()).toBeVisible({ timeout: 10_000 });
    await page.goForward();
    await expect(docRows.first()).toBeVisible({ timeout: 10_000 });
  });

  test("gallery's Logs entry becomes a log card over every run", async ({ page }) => {
    await page.goto(GRID);
    await page.locator(".miller-add").click();
    await page.locator(".gv-row", { hasText: "Logs" }).first().click();
    const col = page.locator(".miller-col").filter({ has: page.locator(".rl-panel") });
    await expect(col).toBeVisible({ timeout: 10_000 });
    await expect(col.locator(".miller-col-title")).toHaveText("Log · everything");
    expect(decodeURIComponent(await page.evaluate(() => location.pathname))).toContain("logView()");
  });

  test("gallery's Unified Search entry becomes a second grid", async ({ page }) => {
    await page.goto(GRID);
    await page.locator(".miller-add").click();
    // By its exact title: "Unified Search (new)" is the Search card.
    await page
      .locator(".gv-row", { has: page.locator(".gv-title", { hasText: /^Unified Search$/ }) })
      .click();
    // Two grid columns now: the default one and the freshly picked one.
    await expect(page.locator(".grid-box .slickgrid-container")).toHaveCount(2, {
      timeout: 10_000,
    });
    expect(decodeURIComponent(await page.evaluate(() => location.pathname))).toContain(
      "gridView()",
    );
  });
});
