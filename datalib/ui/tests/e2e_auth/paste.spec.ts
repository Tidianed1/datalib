// A credential that arrives as text: pasted into the wizard, or stored
// with `latchkey auth set` in a terminal before the app was opened.
// Slack is the example — a built-in latchkey service, so its own
// definition decides how the token is injected.
import { TNG } from "./fake_sites.mjs";
import {
  expect,
  expectGlanceable,
  expectImpersonated,
  pickTile,
  test,
  TILE,
  wizard,
} from "./world";

test("a pasted token is stored, then Check account reaches the workspace", async ({
  page,
  world,
  internet,
}) => {
  await pickTile(page, TILE.slack);
  await wizard(page).getByRole("tab", { name: "Paste a key" }).click();
  const form = wizard(page).locator(".wiz-paste");
  await form.getByLabel("Token").fill(TNG.slackToken);
  await form.getByRole("button", { name: "Store in latchkey" }).click();
  await expect(form).toContainText("Stored in latchkey.");

  // A successful paste checks the account by itself.
  await expect(wizard(page).locator(".wiz-probe-ok")).toContainText("Reached");
  // latchkey's own slack service put the token on the wire…
  for (const r of internet.to("slack.com")) {
    expect(r.headers.authorization).toBe(`Bearer ${TNG.slackToken}`);
  }
  // …and the probe's listings left through curl-impersonate. Not
  // `auth.test`: latchkey's own credential check sends that one too,
  // through the router without datalib's marker, so as a plain curl.
  const listings = internet.to("slack.com").filter((r) => r.path !== "/api/auth.test");
  expect(listings).not.toHaveLength(0);
  for (const r of listings) expectImpersonated(r);
  // …and the config names no secret.
  await wizard(page).getByText("Review the TOML this writes").click();
  await expect(wizard(page).locator(".wiz-review pre")).not.toContainText(TNG.slackToken);
  expect(world.curlCalls().length).toBeGreaterThan(0);
});

test("a token stored on the command line beforehand just works", async ({ page, world }) => {
  world.latchkey("auth", "set", "slack", "-H", `Authorization: Bearer ${TNG.slackToken}`);

  await pickTile(page, TILE.slack);
  await wizard(page).getByRole("button", { name: "Check account" }).click();
  await expect(wizard(page).locator(".wiz-probe-ok")).toContainText("Reached");
});

test("a wrong token fails Check account in a sentence", async ({ page }) => {
  await pickTile(page, TILE.slack);
  await wizard(page).getByRole("tab", { name: "Paste a key" }).click();
  const form = wizard(page).locator(".wiz-paste");
  await form.getByLabel("Token").fill("xoxp-tng-wrong");
  await form.getByRole("button", { name: "Store in latchkey" }).click();

  const failed = wizard(page).locator(".wiz-probe-failed");
  await expect(failed).toBeVisible();
  expectGlanceable(await failed.locator(".wiz-probe-headline").textContent(), "the headline");
});
