// "Sign in with browser" end to end: the backend registers the service
// latchkey does not ship, finds a browser, and runs `auth browser`; a
// real (headless) Chromium loads the fake site's login page, latchkey
// captures the credential, and Check connection then uses it.
//
// The prep steps are load-bearing here, not set up by the harness:
// latchkey's own `ensure-browser` picks the browser (fake_node.mjs only
// makes what it picked headless and points it at the fake internet),
// and `services register` is what makes `claude-ai` a name latchkey
// knows. Drop either and the login fails.
import { TNG } from "./fake_sites.mjs";
import {
  expect,
  expectGlanceable,
  expectImpersonated,
  pickTile,
  subcommand,
  test,
  TILE,
  wizard,
  wizField,
} from "./world";

const LOGIN_PREP = ["services register", "ensure-browser", "auth browser"];

/// A login is three latchkey runs and a browser start; with four workers
/// busy it has taken ~50s on a laptop.
const LOGIN = { timeout: 90_000 };

test("Claude: a cookie-capture login, then Check connection and Load", async ({
  page,
  world,
  internet,
}) => {
  await pickTile(page, TILE.claude);
  await wizard(page).getByRole("button", { name: "Sign in with browser" }).click();
  await expect(wizard(page)).toContainText("Connected.", LOGIN);

  const runs = world.latchkeyRuns().map(subcommand);
  expect(runs.filter((r) => LOGIN_PREP.includes(r))).toEqual(LOGIN_PREP);
  expect(world.browserFound(), "latchkey's ensure-browser should have found one").toBeTruthy();
  expect(internet.to("claude.ai", "/login")).not.toHaveLength(0);

  await wizard(page).getByRole("button", { name: "Check connection" }).click();
  await expect(wizard(page).locator(".wiz-probe-ok")).toContainText("picard@enterprise.test");
  const api = internet.to("claude.ai").filter((r) => r.path.startsWith("/api/"));
  expect(api).not.toHaveLength(0);
  for (const r of api) {
    expect(r.headers.cookie).toContain(`sessionKey=${TNG.claudeSessionKey}`);
    expectImpersonated(r);
  }

  const conversations = wizField(page, "Only these conversations");
  await conversations.locator(".wiz-load-btn").click();
  await expect(conversations.locator(".wiz-load-done")).toContainText(
    "2 conversations from picard@enterprise.test.",
  );
});

test("ChatGPT: a token-capture login, then Check connection and Load", async ({
  page,
  internet,
}) => {
  await pickTile(page, TILE.chatgpt);
  await wizard(page).getByRole("button", { name: "Sign in with browser" }).click();
  await expect(wizard(page)).toContainText("Connected.", LOGIN);

  await wizard(page).getByRole("button", { name: "Check connection" }).click();
  await expect(wizard(page).locator(".wiz-probe-ok")).toContainText("picard@enterprise.test");
  const api = internet.to("chatgpt.com").filter((r) => r.path.startsWith("/backend-api/"));
  expect(api).not.toHaveLength(0);
  for (const r of api) {
    expect(r.headers.authorization).toBe(`Bearer ${TNG.chatgptAccessToken}`);
    expectImpersonated(r);
  }

  const conversations = wizField(page, "Only these conversations");
  await conversations.locator(".wiz-load-btn").click();
  await expect(conversations.locator(".wiz-load-done")).toContainText(
    "1 conversation from picard@enterprise.test.",
  );
});

test.describe("with no browser to find", () => {
  test.use({ worldOptions: { browser: false } });

  test("the wizard offers to fetch one, rather than naming a command", async ({ page }) => {
    // Fails today: the failure is a paragraph that ends in a terminal
    // command (`… ensure-browser`), on a screen whose job is to run it.
    test.fail();
    await pickTile(page, TILE.claude);
    await wizard(page).getByRole("button", { name: "Sign in with browser" }).click();
    const message = wizard(page).locator(".wiz-tabpanel p.wiz-help").last();
    await expect(message).toContainText("no browser");
    expectGlanceable(await message.textContent(), "the login failure");
    await expect(message).not.toContainText("ensure-browser");
    await expect(wizard(page).getByRole("button", { name: /download/i })).toBeVisible();
  });
});
