// More than one login per service: the account box names which, a
// sign-in stores under that name, and a check runs as it. The same for
// every latchkey source.
import { TNG } from "./fake_sites.mjs";
import { expect, pickTile, subcommand, test, TILE, wizard } from "./world";

/// A browser login has a fresh browser to start and a fake site to
/// reach; with four workers busy that has taken ~50s.
const LOGIN = { timeout: 90_000 };

/// latchkey stores a browser login under a name it has never seen —
/// imbue-ai/latchkey#148 filed every one under the unnamed default, and
/// the backend once seeded a placeholder to get past it. Two names, two
/// credentials, and a check that runs as the one in the box.
test("Claude: two accounts by browser login, checked as the one chosen", async ({
  page,
  world,
}) => {
  await pickTile(page, TILE.claude);
  const box = wizard(page).getByRole("combobox", { name: "Claude account" });
  for (const name of ["picard", "riker"]) {
    await box.fill(name);
    await wizard(page).getByRole("button", { name: "Sign in with browser" }).click();
    await expect(wizard(page)).toContainText(`Connected as ${name}.`, LOGIN);
  }
  const stored = JSON.parse(world.latchkey("auth", "list", "--offline"))["claude-ai"];
  expect(Object.keys(stored).sort()).toEqual(["picard", "riker"]);

  await box.fill("riker");
  await wizard(page).getByRole("button", { name: "Check connection" }).click();
  await expect(wizard(page).locator(".wiz-probe-ok")).toBeVisible();
  const runs: { args: string[] }[] = world.latchkeyRuns();
  const check = [...runs].reverse().find((r) => subcommand(r) === "curl");
  expect(check?.args.slice(0, 2)).toEqual(["--account", "riker"]);
  // The source mirrors the account it was checked as.
  await wizard(page).getByText("Review the TOML this writes").click();
  await expect(wizard(page).locator(".wiz-review pre")).toContainText('account = "riker"');
});

/// Slack had no account box at all. A pasted token goes under the name
/// in it, and a second name is a second workspace login.
test("Slack: two pasted tokens under two names, both offered back", async ({ page, world }) => {
  await pickTile(page, TILE.slack);
  const box = wizard(page).getByRole("combobox", { name: "Slack account" });
  await wizard(page).getByRole("tab", { name: "Paste a key" }).click();
  const form = wizard(page).locator(".wiz-paste");
  for (const name of ["enterprise", "defiant"]) {
    await box.fill(name);
    await form.getByLabel("Token").fill(TNG.slackToken);
    await form.getByRole("button", { name: "Store in latchkey" }).click();
    await expect(form).toContainText("Stored in latchkey.");
  }
  const stored = JSON.parse(world.latchkey("auth", "list", "--offline")).slack;
  expect(Object.keys(stored).sort()).toEqual(["defiant", "enterprise"]);

  await box.fill("");
  await box.click();
  await expect(
    wizard(page).getByRole("listbox", { name: "Slack account" }).getByRole("option"),
  ).toContainText(["defiant", "enterprise"]);
});
