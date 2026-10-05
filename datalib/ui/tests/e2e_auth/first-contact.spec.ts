// What happens before the person has asked for anything. On a mac,
// every latchkey run outside a gateway reads the keychain at startup —
// `services info` included — so what runs here is kept to the one read
// that fills the dialog.
import { expect, expectGlanceable, pickTile, subcommand, test, TILE, wizard } from "./world";

/// Picking a tile reads latchkey to fill the dialog — which sign-in
/// ways there are, which accounts it holds — and nothing more. Storing,
/// registering, opening a browser or reaching the service waits for a
/// click.
test("picking a tile only reads", async ({ page, world, internet }) => {
  await pickTile(page, TILE.slack);
  await expect(wizard(page).getByRole("button", { name: "Check connection" })).toBeVisible();
  await expect(wizard(page).getByRole("tab", { name: "Paste a key" })).toBeVisible();
  expect(world.latchkeyRuns().map(subcommand)).toEqual(["services info"]);
  expect(internet.to("slack.com")).toHaveLength(0);
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
