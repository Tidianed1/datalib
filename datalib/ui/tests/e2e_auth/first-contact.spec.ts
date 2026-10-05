// What happens before the person has asked for anything. On a mac,
// every latchkey run outside a gateway reads the keychain at startup —
// `services info` included — so a run here is a keychain prompt the
// person did not ask for.
import { expect, expectGlanceable, pickTile, subcommand, test, TILE, wizard } from "./world";

test("picking a tile runs no latchkey", async ({ page, world }) => {
  // Fails today: the wizard runs `services info` on pick to learn the
  // sign-in ways and the stored accounts. Flip once it waits for a click.
  test.fail();
  await pickTile(page, TILE.slack);
  await expect(wizard(page).getByRole("button", { name: "Check account" })).toBeVisible();
  await expect(wizard(page).getByRole("tab", { name: "Paste a key" })).toBeVisible();
  expect(world.latchkeyRuns().map(subcommand)).toEqual([]);
});

test.describe("with no bundled runtime", () => {
  test.use({ worldOptions: { runtime: false } });

  test("the wizard says latchkey is missing in a sentence", async ({ page }) => {
    // Fails today: the note lives inside the account picker, and Slack
    // has none — so the Connection section offers no way to sign in and
    // says nothing about why.
    test.fail();
    await pickTile(page, TILE.slack);
    const note = wizard(page).getByText("Couldn’t ask latchkey");
    await expect(note).toBeVisible();
    expectGlanceable(await note.textContent(), "the accounts note");
  });
});
