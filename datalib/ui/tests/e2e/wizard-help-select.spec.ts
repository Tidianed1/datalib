// Help text in the Add Data Source wizard can be selected and copied.
// Every field is a <label>, and in WebKit — the desktop app's engine —
// the click that ends a drag across the help activated the label: focus
// jumped to the input and the selection vanished. Chromium skips label
// activation after a selection, so this spec earns its place in the
// webkit project.

import { test, expect } from "@playwright/test";

test("dragging across a field's help keeps the selection", async ({ page }) => {
  await page.goto("/data_sources");
  await page.getByRole("button", { name: "+ Data Source" }).click();
  await page.getByRole("searchbox").fill("messages");
  await page.getByRole("button", { name: /Apple Messages/ }).click();

  const wizard = page.getByRole("dialog");
  const help = wizard.locator(".wiz-help", { hasText: "select chat.db and chat.db-wal together" });
  await expect(help).toBeVisible();

  // The folder the picker opens at is named in the help, copyable.
  await expect(wizard.getByRole("button", { name: "Copy ~/Library/Messages" })).toBeVisible();

  const box = (await help.boundingBox())!;
  await page.mouse.move(box.x + box.width * 0.55, box.y + 5);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width * 0.3, box.y + box.height - 5, { steps: 8 });
  await page.mouse.up();

  await expect
    .poll(() => page.evaluate(() => String(window.getSelection()).length))
    .toBeGreaterThan(20);
  await expect(wizard.locator("input.wiz-path")).not.toBeFocused();
});
